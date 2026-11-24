"""End to end tests: fake node, real fork server, real forked plugin processes."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

from conftest import (
    TIMEOUT,
    Control,
    FakeNode,
    ForkServerProcess,
    Lines,
    PluginProc,
    chain_text,
    default_llm,
    make_event,
)


def kinds(proc) -> dict[str, str]:
    return {h["id"].rsplit(".", 1)[-1]: h["kind"] for h in proc.hello["plugin"]["handlers"]}


def test_command_plugin(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("cmd_plugin")
            assert p.hello["plugin"]["name"] == "cmd_plugin"
            assert p.hello["plugin"]["version"] == "1.2.3"
            echo = next(h for h in p.hello["plugin"]["handlers"] if h["id"].endswith("echo"))
            assert echo["kind"] == "command" and echo["pattern"] == "echo" and echo["priority"] == 0

            r = await p.event("echo hi")
            assert r["matched"] is True
            assert [chain_text(s) for s in r["sends"]] == ["echo:hi"]
            assert r["sends"][0]["session"] == "qq_official:FriendMessage:u1"
            assert r["done"] == {"op": "done", "event_id": r["event_id"], "stop": False, "call_llm": False}

            r = await p.event("复读 你好")
            assert [chain_text(s) for s in r["sends"]] == ["echo:你好"]

            r = await p.event("add 2 3")
            assert chain_text(r["sends"][0]) == "5"
            r = await p.event("add 2")
            assert chain_text(r["sends"][0]) == "12"
            r = await p.event("add x")
            assert r["matched"] is True and "参数" in chain_text(r["sends"][0])

            r = await p.event("whoami", group=True, platform="weixin_ilink")
            assert (
                chain_text(r["sends"][0]) == "u1|Alice|weixin_oc|GROUP_MESSAGE|False|weixin_ilink:GroupMessage:g1|True"
            )
            await p.wait_log("whoami answered")

            r = await p.event("secret")
            assert r["matched"] is True and chain_text(r["sends"][0]) == "你没有权限使用这个指令"
            assert r["done"]["stop"] is True
            r = await p.event("secret", admin=True)
            assert chain_text(r["sends"][0]) == "admin only"

            r = await p.event("quiet")
            assert r["sends"] == [] and r["done"]["stop"] is True and r["done"]["call_llm"] is False

            r = await p.event("hello there")
            assert r["matched"] is False and r["done"]["call_llm"] is True

            r = await p.event("crash")
            assert r["matched"] is True and r["sends"] == []
            await p.wait_log("RuntimeError: boom")

    run(scenario())


def test_command_releases_event_without_a_model_callback(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("cold_cmd_plugin")
            assert [chain_text(s) for s in (await p.event("pond fish"))["sends"]] == ["pond:fish"]
            assert [chain_text(s) for s in (await p.event("event_released"))["sends"]] == ["True"]

    run(scenario())


def test_managers_survive_restart(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            data_dir = os.path.join(node.tmp, "manager-data")
            p = await node.spawn("managers_plugin", data_dir=data_dir)
            assert not (Path(data_dir) / "knowledge_base" / "kb.db").exists()
            created = await p.event("populate")
            assert created["sends"]
            assert not (await p.event("managers"))["done"]["call_llm"]
            await p.write({"op": "shutdown"})
            await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            node.procs.remove(p)
            p = await node.spawn("managers_plugin", data_dir=data_dir)
            restored = await p.event("managers")
            assert [chain_text(s) for s in restored["sends"]] == [
                "Talk about fish.|Fish profile|Fish sleep in the pond."
            ]
            cron = await p.event("cron_basic")
            assert [chain_text(s) for s in cron["sends"]] == ["cron completed"]
            assert [chain_text(s) for s in p.proactive] == ["scheduled fish"]

    run(scenario())


def test_empty_kb_opens_once_on_concurrent_use_after_restart(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            data_dir = os.path.join(node.tmp, "empty-kb-data")
            for _ in range(2):
                p = await node.spawn("managers_plugin", data_dir=data_dir)
                opened = await p.event("open_empty_kb")
                assert [chain_text(s) for s in opened["sends"]] == ["False|True|0|0"], p.logs
                await p.write({"op": "shutdown"})
                exited = await node.control.lines.next(
                    lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id
                )
                assert exited["code"] == 0
                node.procs.remove(p)

    run(scenario())


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="FAISS preload requires Linux")
def test_faiss_preload_is_inherited(monkeypatch, run):
    monkeypatch.setenv("FISH_SIDECAR_WARM_FAISS", "1")
    server = ForkServerProcess()
    try:
        assert len(list(Path(f"/proc/{server.proc.pid}/task").iterdir())) == 1

        async def scenario():
            async with FakeNode(server) as node:
                p = await node.spawn("managers_plugin", share_key="faiss-test")
                inherited = await p.event("faiss_imported")
                assert [chain_text(s) for s in inherited["sends"]] == ["True"], server.output()
                assert (await p.event("populate"))["sends"], p.logs
                restored = await p.event("managers")
                assert [chain_text(s) for s in restored["sends"]] == [
                    "Talk about fish.|Fish profile|Fish sleep in the pond."
                ]

        run(scenario())
    finally:
        server.stop()


def test_plain_text_parser_preserves_native_output(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("managers_plugin")
            result = await p.event("compare_txt")
            assert [chain_text(s) for s in result["sends"]] == ["text matches native parser"], p.logs

    run(scenario())


def test_scheduled_agent_resumes_after_plugin_restart(fork_server, run):
    def model(request):
        if request["contexts"][-1]["role"] == "tool":
            return {"text": "Reminder delivered."}
        assert "send_message_to_user" in {tool["name"] for tool in request["tools"]}
        return {
            "tool_calls": [
                {
                    "id": "reminder-call",
                    "name": "send_message_to_user",
                    "arguments": {
                        "messages": [{"type": "plain", "text": "Fish reminder"}],
                    },
                }
            ]
        }

    async def scenario():
        async with FakeNode(fork_server) as node:
            data_dir = os.path.join(node.tmp, "scheduled-data")
            p = await node.spawn("managers_plugin", data_dir=data_dir)
            scheduled = await p.event("cron_once")
            assert scheduled["sends"]
            await p.write({"op": "shutdown"})
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] == 0
            node.procs.remove(p)
            p = await node.spawn("managers_plugin", llm=model, data_dir=data_dir)
            delivered = await p.recv(lambda m: m.get("op") == "send_to")
            assert delivered["session"] == "qq_official:FriendMessage:u1"
            assert chain_text(delivered) == "Fish reminder"
            await p.recv(lambda m: m.get("op") == "llm" and m["contexts"][-1]["role"] == "tool")

    run(scenario())


def test_recv_keeps_out_of_order_messages(run):
    async def scenario():
        reader = asyncio.StreamReader()
        first = {"op": "done", "event_id": "other"}
        second = {"op": "matched", "event_id": "wanted"}
        reader.feed_data((json.dumps(first) + "\n" + json.dumps(second) + "\n").encode())
        proc = PluginProc("fixture", 0, Lines(reader), None, {}, default_llm)
        assert await proc.recv(lambda m: m["event_id"] == "wanted") == second
        assert await proc.recv(lambda m: m["event_id"] == "other") == first

    run(scenario())


def test_command_group(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("group_plugin")
            assert kinds(p)["math"] == "command_group"
            assert kinds(p)["math_add"] == "command"
            r = await p.event("math add 2 5")
            assert [chain_text(s) for s in r["sends"]] == ["sum=7"]
            r = await p.event("m add 1 1")
            assert [chain_text(s) for s in r["sends"]] == ["sum=2"]
            r = await p.event("math adv pow 2 10")
            assert [chain_text(s) for s in r["sends"]] == ["pow=1024"]
            r = await p.event("math")
            assert r["matched"] is True and "add" in chain_text(r["sends"][0])

    run(scenario())


def test_regex_and_message_type(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("regex_plugin")
            assert kinds(p)["ping"] == "regex"
            assert kinds(p)["count_group"] == "event_message_type"
            r = await p.event("ping42", wake=False)
            assert [chain_text(s) for s in r["sends"]] == ["pong42", " twice"]
            assert r["sends"][1]["chain"][0] == {"type": "at", "user_id": "u1"}

            r = await p.event("random words", group=True, wake=False)
            assert r["matched"] is True and r["sends"] == [] and r["done"]["call_llm"] is True

            r = await p.event("push")
            assert r["sends"] == [] and r["done"]["stop"] is True
            assert p.proactive and p.proactive[0]["session"] == "qq_official:FriendMessage:u1"
            assert chain_text(p.proactive[0]) == "pushed"

    run(scenario())


def test_llm_paths(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("llm_plugin")
            assert kinds(p)["add_persona"] == "llm_request"
            assert kinds(p)["mark_origin"] == "llm_request"

            r = await p.event("ask what is fish")
            assert [chain_text(s) for s in r["sends"]] == ["LLM[be short|hooked:plugin](what is fish)!"]
            assert r["done"]["call_llm"] is False
            assert p.llm_requests[-1]["system_prompt"] == "be short|hooked:plugin"

            r = await p.event("chat hi there")
            assert [chain_text(s) for s in r["sends"]] == ["chat:LLM[sys](hi there)"]
            req = p.llm_requests[-1]
            assert req["contexts"] == [{"role": "user", "content": "earlier"}]

            r = await p.event("fail")
            assert [chain_text(s) for s in r["sends"]] == ["failed:LLMError"]

            r = await p.event("tell me a joke")
            assert r["matched"] is False and r["done"]["call_llm"] is True
            hooked = await p.llm_hook(
                r["event_id"], {"prompt": "tell me a joke", "system_prompt": "base", "contexts": []}
            )
            assert hooked == {"prompt": "tell me a joke", "system_prompt": "base|hooked:chat", "contexts": []}

            unknown = await p.llm_hook("missing", {"prompt": "x", "system_prompt": "s", "contexts": []})
            assert unknown["system_prompt"] == "s"

    run(scenario())


def test_conf_schema_kv_and_hooks(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("conf_plugin", config={"times": 9, "nested": {"name": "x"}})
            assert p.hello["plugin"]["version"] == "v0.4.0"
            r = await p.event("conf")
            assert [chain_text(s) for s in r["sends"]] == ["hello|9|False|7~"]

            await p.write({"op": "config", "config": {"greeting": "yo"}})
            r = await p.event("conf")
            assert chain_text(r["sends"][0]) == "yo|3|False|7~"

            r = await p.event("admins", admins=["A1", "B2"])
            assert chain_text(r["sends"][0]) == "A1,B2~"
            r = await p.event("admins")
            assert chain_text(r["sends"][0]) == "-~"

            r = await p.event("kv a")
            assert chain_text(r["sends"][0]) == "none->a|conf_plugin|True~"
            r = await p.event("kv b")
            assert chain_text(r["sends"][0]) == "a->b|conf_plugin|True~"

            r = await p.event("hello", wake=False)
            hooked = await p.llm_hook(
                r["event_id"],
                {"prompt": "hello", "system_prompt": "sys", "contexts": [{"role": "user", "content": "c"}]},
            )
            assert hooked["system_prompt"] == "yo sys"
            assert hooked["prompt"] == "hello\n[extra]"
            assert hooked["contexts"] == [{"role": "user", "content": "c"}]

    run(scenario())


def test_legacy_kv_file_is_migrated(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            data_dir = os.path.join(node.tmp, "legacy", "data")
            os.makedirs(data_dir)
            legacy = {"plugin\x1ffish/conf_plugin\x1flast": "old", "global\x1fglobal\x1fg": 1}
            with open(os.path.join(data_dir, "fish_kv.json"), "w", encoding="utf-8") as f:
                json.dump(legacy, f)
            p = await node.spawn("conf_plugin", data_dir=data_dir)
            assert any("fish_kv.json" in m["message"] for m in p.logs), p.logs
            r = await p.event("kv new")
            assert chain_text(r["sends"][0]).startswith("old->new|")
            assert not os.path.exists(os.path.join(data_dir, "fish_kv.json"))
            assert os.path.exists(os.path.join(data_dir, "fish_kv.json.migrated"))

    run(scenario())


def test_relative_import(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("relimport_plugin")
            r = await p.event("rel")
            assert chain_text(r["sends"][0]) == "from-lib"

    run(scenario())


def test_plugin_waits_for_memory_limit_before_loading(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            proc_id, reply, conn = await node.spawn_raw("broken_plugin", wait_for_memory_limit=True)
            assert reply["op"] == "spawned"
            reader, writer = await asyncio.wait_for(conn, TIMEOUT)
            try:
                with pytest.raises(TimeoutError):
                    await asyncio.wait_for(reader.readline(), 0.2)
                writer.write(b"\x01")
                await writer.drain()
                message = await Lines(reader).next(lambda m: m.get("level") == "error")
                assert "plugin load failed" in message["message"]
                exited = await node.control.lines.next(
                    lambda m: m.get("op") == "exited" and m.get("proc_id") == proc_id
                )
                assert exited["code"] == 2
            finally:
                writer.close()
                await writer.wait_closed()

    run(scenario())


def test_load_failure_and_exit_codes(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            proc_id, reply, conn = await node.spawn_raw("broken_plugin")
            assert reply["op"] == "spawned"
            reader, writer = await asyncio.wait_for(conn, TIMEOUT)
            msgs = [json.loads(line) async for line in reader]
            errors = [m for m in msgs if m.get("op") == "log" and m.get("level") == "error"]
            assert errors and "ImportError" in errors[0]["message"]
            assert "astrbot.core.pipeline" in errors[0]["message"]
            assert not any(m.get("op") == "hello" for m in msgs)
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == proc_id)
            assert exited["code"] == 2 and exited["pid"] == reply["pid"]

            _, reply, _ = await node.spawn_raw("missing", plugin_dir="/nonexistent/plugin")
            assert reply["op"] == "error"

            p = await node.spawn("cmd_plugin")
            await node.control.write({"op": "kill", "proc_id": p.proc_id})
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] == 0
            node.procs.remove(p)

            p = await node.spawn("cmd_plugin")
            os.kill(p.pid, signal.SIGKILL)
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] == -signal.SIGKILL
            node.procs.remove(p)

            p = await node.spawn("cmd_plugin")
            await p.write({"op": "shutdown"})
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] == 0
            node.procs.remove(p)

    run(scenario())


def test_exited_is_queued_while_disconnected(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("cmd_plugin")
            await node.control.close()
            os.kill(p.pid, signal.SIGKILL)
            await asyncio.sleep(0.5)
            node.control = await Control.connect(fork_server.socket)
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] == -signal.SIGKILL
            node.procs.remove(p)

    run(scenario())


def test_sigterm_stops_server_and_children(run):
    server = ForkServerProcess()
    try:

        async def scenario():
            node = FakeNode(server)
            await node.__aenter__()
            p = await node.spawn("cmd_plugin")
            server.proc.send_signal(signal.SIGTERM)
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == p.proc_id)
            assert exited["code"] in (0, -signal.SIGTERM)
            await node.control.close()
            shutil.rmtree(node.tmp, ignore_errors=True)
            return p.pid

        pid = run(scenario())
        assert server.proc.wait(10) == 0
        assert not os.path.exists(server.socket)
        time.sleep(0.1)
        try:
            os.kill(pid, 0)
            alive = True
        except ProcessLookupError:
            alive = False
        assert not alive
    finally:
        server.stop()


def test_gengwiki_main_imports(tmp_path):
    """Imports a copy of the gengwiki plugin main.py against the shim in a fresh interpreter."""
    source = Path("/Users/niox/project/astrbot_plugin_gengwiki")
    if not (source / "main.py").exists():
        import pytest

        pytest.skip("gengwiki checkout not present")
    plugin = tmp_path / "astrbot_plugin_gengwiki"
    plugin.mkdir()
    for item in source.iterdir():
        if item.suffix in (".py", ".json", ".yaml", ".yml") and item.is_file():
            (plugin / item.name).write_bytes(item.read_bytes())
    script = tmp_path / "check.py"
    script.write_text(
        """
import sys, types
from pathlib import Path
from unittest import mock

for name in ("gengwiki", "PIL"):
    try:
        __import__(name)
    except ImportError:
        mod = mock.MagicMock(name=name)
        mod.__path__ = []
        sys.modules[name] = mod
        sys.modules[name + ".Image"] = mod.Image
from fish_sidecar import loader
plugin_dir = Path(sys.argv[1])
package, main = loader.import_plugin_package(plugin_dir, plugin_dir.name)
md = loader.find_star(package, main)
import json
from astrbot.core.star.star_handler import star_handlers_registry
from fish_sidecar import tools
from fish_sidecar.dispatch import handler_descriptor
print("STAR", md.star_cls_type.__name__)
for h in star_handlers_registry:
    d = handler_descriptor(h)
    print("HANDLER", h.event_type.name, h.handler_name, d and d["kind"])
for t in tools.plugin_tools():
    print("TOOL", json.dumps(tools.tool_schema(t), ensure_ascii=False))
"""
    )
    env = dict(os.environ)
    env["PYTHONPATH"] = str(Path(__file__).resolve().parent.parent)
    out = subprocess.run(
        [sys.executable, str(script), str(plugin)],
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
        cwd=str(tmp_path),
    )
    print(out.stdout)
    assert out.returncode == 0, out.stderr
    assert "STAR " in out.stdout
    assert "HANDLER" in out.stdout
    import json as _json

    tools = {
        t["name"]: t for t in (_json.loads(line[5:]) for line in out.stdout.splitlines() if line.startswith("TOOL "))
    }
    assert "search_memes" in tools
    assert tools["search_memes"]["parameters"]["properties"]["q"]["type"] == "string"


def test_plugin_home_is_inside_data_dir(tmp_path):
    plugin_dir = tmp_path / "src"
    plugin_dir.mkdir()
    data_dir = tmp_path / "data"
    code = (
        "import os, sys\n"
        "from pathlib import Path\n"
        "from fish_sidecar import loader\n"
        "loader.prepare_paths(Path(sys.argv[1]), None, Path(sys.argv[2]), 'p')\n"
        "(Path.home() / '.p').mkdir()\n"
        "print(Path.home(), os.environ['XDG_CACHE_HOME'])\n"
    )
    env = {**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1]), "HOME": "/nonexistent"}
    out = subprocess.run(
        [sys.executable, "-c", code, str(plugin_dir), str(data_dir)],
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
    )
    assert out.returncode == 0, out.stderr
    home = (data_dir / "home").resolve()
    assert out.stdout.split() == [str(home), str(home / ".cache")]
    assert (home / ".p").is_dir()


def test_llm_tools(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("tool_plugin")
            tools = {h["pattern"]: h["tool"] for h in p.hello["plugin"]["handlers"] if h["kind"] == "llm_tool"}
            assert set(tools) == {"lookup", "sticker", "boom"}
            assert tools["lookup"] == {
                "name": "lookup",
                "description": "Looks a word up.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "q": {"type": "string", "description": "the word\nto look up"},
                        "limit": {"type": "number", "description": "how many results"},
                    },
                },
            }
            assert tools["sticker"]["parameters"]["properties"] == {}

            r = await p.event("hello")
            event_id = r["event_id"]
            await p.llm_hook(event_id, {"prompt": "hello"})

            async def call(name, arguments, call_id):
                await p.write(
                    {
                        "op": "tool_call",
                        "call_id": call_id,
                        "event_id": event_id,
                        "event": {},
                        "name": name,
                        "arguments": arguments,
                    }
                )
                sends = []
                while True:
                    m = await p.recv(
                        lambda m: (
                            (m.get("op") == "tool_result" and m.get("call_id") == call_id) or m.get("op") == "send"
                        )
                    )
                    if m["op"] == "send":
                        sends.append(m)
                    else:
                        return m, sends

            res, sends = await call("lookup", {"q": "梗", "extra": 1}, "c1")
            assert (res["text"], res["sent"], sends) == ("梗:3:yes", False, [])
            res, sends = await call("sticker", {"caption": "cat"}, "c2")
            assert res["sent"] is True and res["text"] == "工具已把内容直接发给用户"
            assert [chain_text(s) for s in sends] == ["[sticker cat]"] and sends[0]["event_id"] == event_id
            res, _ = await call("sticker", {"caption": "none"}, "c3")
            assert (res["text"], res["sent"]) == ("no such sticker", False)
            res, _ = await call("boom", {}, "c4")
            assert "kaput" in res["error"]
            res, _ = await call("missing", {}, "c5")
            assert "missing" in res["error"]

    run(scenario())


def test_idle_check_reports_background_work(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:

            async def idle(p):
                await p.write({"op": "idle_check"})
                return (await p.recv(lambda m: m.get("op") == "idle_result"))["busy"]

            quiet = await node.spawn("cmd_plugin")
            assert await idle(quiet) == []
            assert (await quiet.event("echo hi"))["matched"]
            assert await idle(quiet) == []

            bg = await node.spawn("bg_plugin")
            assert await idle(bg) == []
            assert (await bg.event("bgstart"))["matched"]
            busy = await idle(bg)
            assert any("后台任务" in r for r in busy) and any("定时器" in r for r in busy), busy
            assert (await bg.event("bgstop"))["matched"]
            assert await idle(bg) == []

    run(scenario())


TEMPLATES = sys.platform.startswith("linux")


def parent_pid(pid: int) -> int:
    stat = Path(f"/proc/{pid}/stat").read_text()
    return int(stat.rsplit(")", 1)[1].split()[1])


def test_shared_install_forks_from_one_template(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            a = await node.spawn("share_plugin", {"name": "a"}, share_key="share-1")
            b = await node.spawn("share_plugin", {"name": "b"}, share_key="share-1")
            assert len(node.control.templates) == (1 if TEMPLATES else 0)
            replies = []
            for p in (a, b):
                r = await p.event("where")
                replies.append(chain_text(r["sends"][0]).split("|"))
                await p.wait_log("share plugin imported")
                await p.wait_log("share plugin printed")
            (cwd_a, home_a, data_a, name_a, forked_a), (cwd_b, _, data_b, name_b, _) = replies
            assert (name_a, name_b) == ("a", "b")
            assert cwd_a != cwd_b and data_a != data_b
            assert cwd_a.startswith(os.path.realpath(node.tmp)) and home_a == os.path.join(cwd_a, "home")
            assert forked_a == str(TEMPLATES)
            if TEMPLATES:
                assert len(list(Path(f"/proc/{fork_server.proc.pid}/task").iterdir())) == 1
                assert parent_pid(a.pid) == fork_server.proc.pid
            await node.control.write({"op": "kill", "proc_id": a.proc_id})
            exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == a.proc_id)
            assert exited["pid"] == a.pid
            node.procs.remove(a)
            r = await b.event("where")
            assert chain_text(r["sends"][0]).split("|")[3] == "b"

    run(scenario())


@pytest.mark.skipif(not TEMPLATES, reason="templates need Linux")
@pytest.mark.parametrize(
    ("plugin", "command", "reason"),
    [
        ("cwd_plugin", "cwd", "工作目录"),
        ("seed_plugin", "token", "随机数"),
        ("thread_plugin", None, "线程"),
        ("clock_plugin", "uptime", "导入时的时间"),
        ("pid_plugin", "pid", "模板进程号"),
    ],
)
def test_import_that_cannot_be_shared_loads_directly(fork_server, run, plugin, command, reason):
    async def scenario():
        async with FakeNode(fork_server) as node:
            first = await node.spawn(plugin, share_key=f"{plugin}-1")
            note = await first.wait_log("未与其他机器人共享导入")
            assert reason in note["message"]
            templates = len(node.control.templates)
            second = await node.spawn(plugin, share_key=f"{plugin}-1")
            await second.wait_log("未与其他机器人共享导入")
            assert len(node.control.templates) == templates
            for p in (first, second):
                assert parent_pid(p.pid) == fork_server.proc.pid
                if command:
                    r = await p.event(command)
                    assert r["sends"]
            if plugin == "seed_plugin":
                tokens = {chain_text((await p.event(command))["sends"][0]) for p in (first, second)}
                assert len(tokens) == 2
            if plugin == "pid_plugin":
                for p in (first, second):
                    owner, current = chain_text((await p.event(command))["sends"][0]).split("|")
                    assert owner == current

    run(scenario())


@pytest.mark.skipif(not TEMPLATES, reason="templates need Linux")
def test_process_local_random_keys_share_the_template(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            procs = [await node.spawn("otel_plugin", share_key="otel-1") for _ in range(2)]
            assert len(node.control.templates) == 1
            replies = [chain_text((await p.event("key"))["sends"][0]).split("|") for p in procs]
            assert replies[0][0] == replies[1][0]
            assert replies[0][1] != replies[1][1]
            for p in procs:
                assert not [m for m in p.logs if "未与其他机器人共享导入" in m.get("message", "")]

    run(scenario())


def test_direct_plugin_keeps_preloaded_objects_frozen(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("pid_plugin")
            r = await p.event("frozen")
            assert int(chain_text(r["sends"][0])) > 0

    run(scenario())


@pytest.mark.skipif(not TEMPLATES, reason="templates need Linux")
def test_template_disabled_by_env(run, monkeypatch):
    monkeypatch.setenv("FISH_SIDECAR_TEMPLATES", "0")
    server = ForkServerProcess()
    try:

        async def scenario():
            async with FakeNode(server) as node:
                p = await node.spawn("share_plugin", {"name": "x"}, share_key="share-2")
                assert node.control.templates == []
                r = await p.event("where")
                assert chain_text(r["sends"][0]).split("|")[3:] == ["x", "False"]

        run(scenario())
    finally:
        server.stop()


@pytest.mark.skipif(not TEMPLATES, reason="templates need Linux")
def test_kill_before_template_fork_cancels_spawn(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            assert node.control is not None
            await node.control.write(
                {
                    "op": "spawn",
                    "proc_id": "slow1",
                    "plugin_dir": str(Path(__file__).parent / "plugins" / "slow_plugin"),
                    "data_dir": os.path.join(node.tmp, "slow1"),
                    "plugin_name": "slow_plugin",
                    "socket": os.path.join(node.tmp, "slow1.sock"),
                    "share_key": "slow-1",
                    "config_on_socket": True,
                }
            )
            await asyncio.sleep(0.3)
            await node.control.write({"op": "kill", "proc_id": "slow1"})
            reply = await node.control.lines.next(lambda m: m.get("proc_id") == "slow1")
            assert reply["op"] in ("error", "spawned")
            if reply["op"] == "spawned":
                exited = await node.control.lines.next(
                    lambda m: m.get("op") == "exited" and m.get("proc_id") == "slow1"
                )
                assert exited["pid"] == reply["pid"]
            p = await node.spawn("slow_plugin", share_key="slow-1")
            assert parent_pid(p.pid) == fork_server.proc.pid

    run(scenario())


def test_import_watch_recognizes_clock_readings_and_pid(tmp_path):
    import datetime
    import os
    import time
    import types

    from fish_sidecar.template import ImportWatch

    watch = ImportWatch(str(tmp_path))
    watch.start()
    module = types.ModuleType("probe_mod")
    module.WALL = time.time()
    module.MS = int(time.time() * 1000)
    module.NOW = datetime.datetime.now()
    module.PID = os.getpid()
    module.LIMITS = {"timeout": 30.0, "retries": 3, "flag": True, "year": 2026}
    watch.stop()
    for name in ("WALL", "MS", "NOW"):
        assert watch.clock_reading(getattr(module, name)), name
    assert watch.clock_reading(datetime.datetime.now(datetime.timezone.utc))
    assert not any(watch.clock_reading(v) for v in module.LIMITS.values())
    assert not watch.clock_reading(time.time() + 3600)
    assert watch.pid_read

    plain = types.ModuleType("plain_mod")
    plain.LIMITS = module.LIMITS
    watch.scan_values([plain])
    assert watch.reasons == []
    watch.scan_values([module])
    assert watch.reasons and "导入时的时间" in watch.reasons[0]

    pid_only = types.ModuleType("pid_mod")
    pid_only.PID = os.getpid()
    other = ImportWatch(str(tmp_path))
    other.scan_values([pid_only])
    assert other.reasons == [], "a pid is reported only when the import read it"
    other.pid_read = True
    other.scan_values([pid_only])
    assert "模板进程号" in other.reasons[0]


def handler_ids(proc: PluginProc) -> set[str]:
    return {h["id"] for h in proc.hello["plugin"]["handlers"]}


def test_group_runs_plugins_with_separate_state(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            group = await node.group()
            cmd = await group.load("cmd_plugin", config={"echo_prefix": "A"})
            conf_dir = os.path.join(node.tmp, "conf", "data")
            conf = await group.load("conf_plugin", config={"times": 9}, data_dir=conf_dir)
            spin = await group.load("spin_plugin")
            assert cmd.hello["plugin"]["name"] == "cmd_plugin"
            assert all(".cmd_plugin." in i or i == "tool:echo_tool" for i in handler_ids(cmd)), handler_ids(cmd)
            assert all(".conf_plugin." in i for i in handler_ids(conf)), handler_ids(conf)
            assert conf.hello["plugin"]["wake_commands"] is None

            r = await cmd.event("echo hi")
            assert [chain_text(s) for s in r["sends"]] == ["A:hi"]
            r = await conf.event("echo hi")
            assert not r["matched"]
            r = await conf.event("conf")
            assert [chain_text(s) for s in r["sends"]] == ["hello|9|False|7~"]
            r = await conf.event("kv a")
            assert chain_text(r["sends"][0]) == "none->a|conf_plugin|True~"
            assert os.path.exists(os.path.join(conf_dir, "data_v4.db"))
            assert os.path.isdir(os.path.join(conf_dir, "plugin_data", "conf_plugin"))
            assert os.path.exists(os.path.join(conf_dir, "config", "conf_plugin_config.json"))

            await cmd.event("whoami")
            await cmd.wait_log("whoami answered")
            await spin.event("say x")
            await spin.wait_log("printed x")
            assert not any("whoami" in m["message"] for m in conf.logs + spin.logs)

            await conf.write({"op": "shutdown"})
            assert (await group.exited(conf.proc_id))["code"] == 0
            again = await group.load("conf_plugin", data_dir=conf_dir)
            r = await again.event("kv b")
            assert chain_text(r["sends"][0]) == "a->b|conf_plugin|True~"
            fresh_dir = os.path.join(node.tmp, "fresh", "data")
            await again.write({"op": "shutdown"})
            await group.exited(again.proc_id)
            fresh = await group.load("conf_plugin", data_dir=fresh_dir)
            r = await fresh.event("kv c")
            assert chain_text(r["sends"][0]) == "none->c|conf_plugin|True~"
            r = await cmd.event("echo still")
            assert [chain_text(s) for s in r["sends"]] == ["A:still"]

            await group.write({"op": "unload", "proc_id": cmd.proc_id})
            assert (await group.exited(cmd.proc_id))["code"] == 0
            await group.write({"op": "exit"})
            done = await node.control.lines.next(
                lambda m: m.get("op") == "exited" and m.get("proc_id") == group.proc_id
            )
            assert done["code"] == 0

    run(scenario())


def test_group_sends_unsuitable_plugins_back(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            group = await node.group()
            await group.load("relimport_plugin")
            for plugin, needle in [
                ("bg_plugin", "后台任务"),
                ("thread_plugin", "线程"),
                ("cwd_plugin", "相对路径"),
                ("twin_plugin", "lib"),
            ]:
                proc_id, lines, _ = await group.load_raw(plugin)
                exited = await group.exited(proc_id)
                assert exited["code"] == 4 and exited["drain"] is False, (plugin, exited)
                logs = []
                try:
                    while True:
                        logs.append(await lines.next(timeout=2))
                except (EOFError, TimeoutError):
                    pass
                assert not any(m.get("op") == "hello" for m in logs), (plugin, logs)
                assert any(needle in m.get("message", "") for m in logs), (plugin, logs)

    run(scenario())


def make_venv(root: str, files: dict[str, str], dists: dict[str, str]) -> str:
    """Writes a site-packages dir with ``files`` and a dist-info dir for each distribution name and version."""
    site = os.path.join(root, "lib", "site-packages")
    os.makedirs(site)
    files = {"_virtualenv.py": "", "_virtualenv.pth": "import _virtualenv\n", **files}
    for rel, text in files.items():
        os.makedirs(os.path.dirname(os.path.join(site, rel)), exist_ok=True)
        Path(site, rel).write_text(text)
    for name, version in dists.items():
        info = Path(site, f"{name}-{version}.dist-info")
        info.mkdir()
        (info / "METADATA").write_text(f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n")
        tops = sorted({rel.split("/")[0].removesuffix(".py") for rel in files if not rel.startswith("_virtualenv")})
        (info / "top_level.txt").write_text("\n".join(tops) + "\n")
    return site


def test_group_takes_plugins_whose_own_packages_fit(fork_server, run):
    import importlib.metadata

    yaml_version = importlib.metadata.version("PyYAML")

    async def scenario():
        async with FakeNode(fork_server) as node:
            group = await node.group()
            first = make_venv(os.path.join(node.tmp, "va"), {"fishvenvdep.py": "VALUE = 'a'\n"}, {"fishvenvdep": "1.0"})
            a = await group.load("venv_plugin", venv=first)
            r = await a.event("dep")
            assert chain_text(r["sends"][0]) == "a"
            same = make_venv(
                os.path.join(node.tmp, "vs"), {"yaml/__init__.py": "", "other.py": ""}, {"PyYAML": yaml_version}
            )
            same_plugin = await group.load("cmd_plugin", venv=same)
            r = await same_plugin.event("echo hi")
            assert r["sends"]
            for files, dists, needle in [
                ({"yaml/__init__.py": ""}, {"PyYAML": "0.0.1"}, "PyYAML 0.0.1"),
                ({"fishvenvdep.py": "VALUE = 'x'\n"}, {"fishvenvdep": "2.0"}, "fishvenvdep 2.0"),
                ({"extra.pth": "import os\n"}, {}, "extra.pth"),
            ]:
                venv = make_venv(tempfile.mkdtemp(dir=node.tmp), files, dists)
                proc_id, lines, _ = await group.load_raw("cmd_plugin", venv=venv)
                exited = await group.exited(proc_id)
                assert exited["code"] == 4, (needle, exited)
                logs = []
                try:
                    while True:
                        logs.append(await lines.next(timeout=2))
                except (EOFError, TimeoutError):
                    pass
                assert any(needle in m.get("message", "") for m in logs), (needle, logs)

            await a.write({"op": "shutdown"})
            assert (await group.exited(a.proc_id))["code"] == 0
            second = make_venv(
                os.path.join(node.tmp, "vb"), {"fishvenvdep.py": "VALUE = 'b'\n"}, {"fishvenvdep": "2.0"}
            )
            b = await group.load("venv_plugin", venv=second)
            r = await b.event("dep")
            assert chain_text(r["sends"][0]) == "b"

    run(scenario())


def test_group_demotes_a_plugin_that_blocks_the_loop(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            group = await node.group()
            spin = await group.load("spin_plugin")
            cmd = await group.load("cmd_plugin")
            r = await spin.event("spin 6")
            assert [chain_text(s) for s in r["sends"]] == ["spun"]
            exited = await group.exited(spin.proc_id)
            assert exited["code"] == 4 and exited["drain"] is False, exited
            await spin.wait_log("阻塞事件循环")
            r = await cmd.event("echo ok")
            assert [chain_text(s) for s in r["sends"]] == ["echo:ok"]
            r = await cmd.event("spin 1")
            assert not r["matched"]

    run(scenario())


def test_every_hook_kind_runs_for_the_node(fork_server, run):
    async def scenario():
        async with FakeNode(fork_server) as node:
            p = await node.spawn("hooks_plugin")
            k = kinds(p)
            assert (k["begin"], k["done"], k["using"], k["responded"], k["response"]) == (
                "agent_begin",
                "agent_done",
                "using_llm_tool",
                "llm_tool_respond",
                "llm_response",
            )
            assert (k["decorate"], k["sent"], k["on_error"], k["loaded"], k["unloaded"]) == (
                "decorating_result",
                "after_message_sent",
                "plugin_error",
                "plugin_loaded",
                "plugin_unloaded",
            )
            assert not any("never called" in m["message"] for m in p.logs), p.logs

            r = await p.event("hi there")
            eid = r["event_id"]
            await p.llm_hook(eid, {"prompt": "hi there", "system_prompt": "s", "contexts": []})
            messages = [{"role": "system", "content": "s"}, {"role": "user", "content": "hi there"}]
            await p.hook("agent_begin", eid, {"messages": messages})
            tool = {"name": "picture", "description": "Draws", "parameters": {"type": "object", "properties": {}}}
            await p.hook("using_llm_tool", eid, {"tool": tool, "arguments": {"q": "cat"}})
            await p.write(
                {
                    "op": "tool_call",
                    "call_id": "t1",
                    "event_id": eid,
                    "event": {},
                    "name": "picture",
                    "arguments": {"q": "cat"},
                }
            )
            res = await p.recv(lambda m: m.get("op") == "tool_result" and m.get("call_id") == "t1")
            assert (res["text"], res["images"]) == ("drew cat", ["data:image/png;base64,AAAA"])
            await p.hook("llm_tool_respond", eid, {"tool": tool, "arguments": {"q": "cat"}, "result": "drew cat"})
            r = await p.hook("llm_response", eid, {"completion_text": "hello[tag]"})
            assert r["payload"] == {"completion_text": "hello"}
            await p.hook("agent_done", eid, {"messages": messages, "completion_text": "hello"})
            r = await p.hook("decorating_result", eid, {"chain": [{"type": "plain", "text": "hello"}], "llm": True})
            chain = r["payload"]["chain"]
            assert chain == [{"type": "plain", "text": "hello"}, {"type": "image", "url": "https://example.com/m.png"}]
            await p.hook("after_message_sent", eid, {"chain": chain, "llm": True})

            error = {"plugin": "other", "handler": "h", "error": "bad", "traceback": ""}
            await p.hook("plugin_error", "unseen", error, event=make_event("x"))
            await p.hook("plugin_loaded", None, {"plugin": {"name": "other", "version": "1"}})
            await p.hook("plugin_unloaded", None, {"plugin": {"name": "hooks_plugin"}})

            await p.event("boom")
            assert p.errors[-1]["handler"] == "boom" and "kaboom" in p.errors[-1]["error"]

            r = await p.event("seen")
            assert chain_text(r["sends"][0]).split(";") == [
                "begin:2",
                "using:picture:cat",
                "respond:picture:drew cat",
                "done:True:hello",
                "sent:2",
                "error:other:h:bad",
                "loaded:other",
                "unloaded:hooks_plugin",
                "error:hooks_plugin:boom:kaboom",
            ]

    run(scenario())


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="malloc_trim needs glibc")
def test_idle_trim_returns_freed_heap():
    from fish_sidecar import memory

    blocks = [bytes(512) for _ in range(100_000)]
    del blocks
    assert memory.free_heap_bytes() > 0
    assert memory.collect_and_trim(min_free=0)
    assert not memory.collect_and_trim(min_free=1 << 60)


def test_group_scan_skips_files_the_plugin_cannot_import(tmp_path):
    from fish_sidecar import group

    (tmp_path / "main.py").write_text("x = 1\n")
    scripts = tmp_path / "webui" / "scripts"
    scripts.mkdir(parents=True)
    (scripts / "build-font.py").write_text("import subprocess\n")
    (tmp_path / "my-tools").mkdir()
    (tmp_path / "my-tools" / "run.py").write_text("import subprocess\n")
    assert group.static_blockers(tmp_path) == []
    (scripts / "build_font.py").write_text("import subprocess\n")
    assert group.static_blockers(tmp_path) == ["子进程 (webui/scripts/build_font.py)"]
