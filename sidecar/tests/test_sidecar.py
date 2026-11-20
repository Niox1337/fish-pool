"""End to end tests: fake node, real fork server, real forked plugin processes."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from conftest import TIMEOUT, Control, FakeNode, ForkServerProcess, Lines, chain_text


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
            assert chain_text(r["sends"][0]) == "u1|Alice|weixin_oc|GROUP_MESSAGE|False|weixin_ilink:GroupMessage:g1|True"
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

            r = await p.event("ask what is fish")
            assert [chain_text(s) for s in r["sends"]] == ["LLM[be short|hooked](what is fish)!"]
            assert r["done"]["call_llm"] is False
            assert p.llm_requests[-1]["system_prompt"] == "be short|hooked"

            r = await p.event("chat hi there")
            assert [chain_text(s) for s in r["sends"]] == ["chat:LLM[sys](hi there)"]
            req = p.llm_requests[-1]
            assert req["contexts"] == [{"role": "user", "content": "earlier"}]

            r = await p.event("fail")
            assert [chain_text(s) for s in r["sends"]] == ["failed:LLMError"]

            r = await p.event("tell me a joke")
            assert r["matched"] is False and r["done"]["call_llm"] is True
            hooked = await p.llm_hook(r["event_id"], {"prompt": "tell me a joke", "system_prompt": "base", "contexts": []})
            assert hooked == {"prompt": "tell me a joke", "system_prompt": "base|hooked", "contexts": []}

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

            r = await p.event("kv a")
            assert chain_text(r["sends"][0]) == "none->a|conf_plugin|True~"
            r = await p.event("kv b")
            assert chain_text(r["sends"][0]) == "a->b|conf_plugin|True~"

            r = await p.event("hello", wake=False)
            hooked = await p.llm_hook(r["event_id"], {"prompt": "hello", "system_prompt": "sys", "contexts": [{"role": "user", "content": "c"}]})
            assert hooked["system_prompt"] == "yo sys"
            assert hooked["prompt"] == "hello\n[extra]"
            assert hooked["contexts"] == [{"role": "user", "content": "c"}]

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
                exited = await node.control.lines.next(lambda m: m.get("op") == "exited" and m.get("proc_id") == proc_id)
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
            assert errors and "NotImplementedError" in errors[0]["message"]
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
md = loader.find_star(package, main.__name__)
from astrbot.core.star.star_handler import star_handlers_registry
from fish_sidecar.dispatch import handler_descriptor
print("STAR", md.star_cls_type.__name__)
for h in star_handlers_registry:
    d = handler_descriptor(h)
    print("HANDLER", h.event_type.name, h.handler_name, d and d["kind"])
"""
    )
    env = dict(os.environ)
    env["PYTHONPATH"] = str(Path(__file__).resolve().parent.parent)
    out = subprocess.run([sys.executable, str(script), str(plugin)], capture_output=True, text=True, env=env, timeout=60, cwd=str(tmp_path))
    print(out.stdout)
    assert out.returncode == 0, out.stderr
    assert "STAR " in out.stdout
    assert "HANDLER" in out.stdout
