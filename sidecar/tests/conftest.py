"""Fake node: drives a real fork server over unix sockets."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

SIDECAR_DIR = Path(__file__).resolve().parent.parent
PLUGINS_DIR = Path(__file__).resolve().parent / "plugins"
TIMEOUT = 15


def short_tmpdir() -> str:
    """Unix socket paths are limited to about 100 bytes, so sockets live under /tmp."""
    return tempfile.mkdtemp(prefix="fishsc", dir="/tmp")


class ForkServerProcess:
    def __init__(self) -> None:
        self.dir = short_tmpdir()
        self.socket = os.path.join(self.dir, "ctl.sock")
        env = dict(os.environ)
        env["PYTHONPATH"] = str(SIDECAR_DIR)
        env.setdefault("OBJC_DISABLE_INITIALIZE_FORK_SAFETY", "YES")
        self.log_path = os.path.join(self.dir, "server.log")
        self.log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(
            [sys.executable, "-m", "fish_sidecar.server", "--socket", self.socket],
            cwd=str(SIDECAR_DIR),
            env=env,
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + TIMEOUT
        while not os.path.exists(self.socket):
            if self.proc.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("fork server did not start:\n" + self.output())
            time.sleep(0.05)

    def output(self) -> str:
        self.log.flush()
        return Path(self.log_path).read_text(errors="replace")

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.log.close()
        shutil.rmtree(self.dir, ignore_errors=True)


class Lines:
    """JSON lines reader with a backlog for messages that were read but not wanted yet."""

    def __init__(self, reader: asyncio.StreamReader, hook: Callable[[dict], bool] | None = None) -> None:
        self.reader = reader
        self.backlog: list[dict[str, Any]] = []
        # Called for every received message. A True result consumes the message.
        self.hook = hook

    async def next(self, pred: Callable[[dict], bool] = lambda m: True, timeout: float = TIMEOUT) -> dict[str, Any]:
        for i, m in enumerate(self.backlog):
            if pred(m):
                return self.backlog.pop(i)
        end = time.monotonic() + timeout
        while True:
            left = end - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"no matching message, backlog={self.backlog}")
            line = await asyncio.wait_for(self.reader.readline(), left)
            if not line:
                raise EOFError(f"socket closed, backlog={self.backlog}")
            m = json.loads(line)
            if self.hook is not None and self.hook(m):
                continue
            if pred(m):
                return m
            self.backlog.append(m)


class Control:
    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.lines = Lines(reader, self.on_message)
        self.writer = writer
        self.templates: list[dict] = []

    def on_message(self, m: dict) -> bool:
        """Confirms template memory groups like the node does when it has no cgroup support."""
        if m.get("op") != "template":
            return False
        self.templates.append(m)
        reply = {"op": "template_ready", "key": m["key"], "pid": m["pid"], "ok": True}
        self.writer.write(json.dumps(reply).encode() + b"\n")
        return True

    @classmethod
    async def connect(cls, path: str) -> Control:
        reader, writer = await asyncio.open_unix_connection(path, limit=64 * 1024 * 1024)
        return cls(reader, writer)

    async def write(self, message: dict) -> None:
        self.writer.write(json.dumps(message).encode() + b"\n")
        await self.writer.drain()

    async def close(self) -> None:
        self.writer.close()
        try:
            await self.writer.wait_closed()
        except Exception:
            pass


def default_llm(message: dict) -> dict:
    prompt = message.get("prompt") or ""
    if "fail" in prompt:
        return {"error": "llm unavailable"}
    return {"text": f"LLM[{message.get('system_prompt', '')}]({prompt})"}


# Plugin messages the fake node answers by itself while waiting for something else.
SERVICE_OPS = ("llm", "log", "send_to", "plugin_error", "result", "agent")


class PluginProc:
    """Node side of one plugin process socket."""

    def __init__(
        self,
        proc_id: str,
        pid: int,
        lines: Lines,
        writer: asyncio.StreamWriter,
        hello: dict,
        llm: Callable[[dict], dict],
    ) -> None:
        self.proc_id = proc_id
        self.pid = pid
        self.lines = lines
        self.writer = writer
        self.hello = hello
        self.llm = llm
        self.llm_requests: list[dict] = []
        self.logs: list[dict] = []
        self.proactive: list[dict] = []
        self.errors: list[dict] = []

    async def write(self, message: dict) -> None:
        self.writer.write(json.dumps(message, ensure_ascii=False).encode() + b"\n")
        await self.writer.drain()

    async def _service(self, m: dict) -> bool:
        """Handles messages that are not part of an event exchange. Returns True when consumed."""
        if m.get("op") == "llm":
            self.llm_requests.append(m)
            await self.write({"op": "llm_result", "req_id": m["req_id"], **self.llm(m)})
            return True
        if m.get("op") == "log":
            self.logs.append(m)
            return True
        if m.get("op") == "send_to":
            self.proactive.append(m)
            return True
        if m.get("op") == "plugin_error":
            self.errors.append(m)
            return True
        if m.get("op") == "result":
            sent = await self.deliver(m.get("event_id"), m.get("chain") or [], bool(m.get("llm")), m.get("session"))
            await self.reply({"op": "result_done", "req_id": m["req_id"], "sent": sent})
            return True
        if m.get("op") == "agent":
            await self.reply({"op": "agent_result", "req_id": m["req_id"], **(await self.agent(m))})
            return True
        return False

    async def reply(self, message: dict) -> None:
        """Answers a plugin request. A plugin that already exited gets no answer."""
        try:
            await self.write(message)
        except ConnectionError:
            pass

    async def hook(self, kind: str, event_id: str | None, payload: dict, event: dict | None = None) -> dict:
        """Runs one hook kind in the plugin like the node does and returns the hook_result message."""
        hook_id = uuid.uuid4().hex[:12]
        message = {"op": "hook", "hook_id": hook_id, "kind": kind, "event_id": event_id, "payload": payload}
        if event is not None:
            message["event"] = event
        try:
            await self.write(message)
            return await self.recv(lambda m: m.get("op") == "hook_result" and m.get("hook_id") == hook_id)
        except (EOFError, ConnectionError):
            return {}

    async def deliver(self, event_id: str | None, chain: list, llm: bool, session: str | None = None) -> bool:
        """Decorates and sends a result like the node does. The send is queued as a ``send`` message."""
        r = await self.hook("decorating_result", event_id, {"chain": chain, "llm": llm})
        chain = (r.get("payload") or {}).get("chain", chain)
        if not chain:
            return False
        self.lines.backlog.append({"op": "send", "event_id": event_id, "session": session, "chain": chain})
        await self.hook("after_message_sent", event_id, {"chain": chain, "llm": llm})
        return True

    async def agent(self, m: dict) -> dict:
        """Runs a plugin LLM request like the node's agent, without tools."""
        event_id = m.get("event_id")
        request = {k: m[k] for k in ("prompt", "system_prompt", "contexts", "image_urls") if k in m}
        r = await self.hook("llm_request", event_id, {**request, "plugin_request": True})
        if r.get("stopped"):
            return {"stopped": True}
        hooked = r.get("payload") or request
        self.llm_requests.append(hooked)
        reply = self.llm(hooked)
        if reply.get("error"):
            return {"error": reply["error"]}
        r = await self.hook("llm_response", event_id, {"completion_text": reply.get("text", "")})
        text = (r.get("payload") or {}).get("completion_text", reply.get("text", ""))
        await self.deliver(event_id, [{"type": "plain", "text": text}], True, m.get("session"))
        return {}

    async def recv(self, pred: Callable[[dict], bool], timeout: float = TIMEOUT) -> dict:
        end = time.monotonic() + timeout
        while True:
            remaining = end - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"no matching plugin message, backlog={self.lines.backlog}")
            m = await self.lines.next(
                lambda m: m.get("op") in SERVICE_OPS or pred(m),
                remaining,
            )
            await self._service(m)
            if pred(m):
                return m

    async def event(self, text: str, **kw: Any) -> dict[str, Any]:
        """Sends one event and collects matched, sends and done."""
        event_id = uuid.uuid4().hex[:12]
        await self.write({"op": "event", "event_id": event_id, "event": make_event(text, **kw)})
        matched = await self.recv(lambda m: m.get("op") == "matched" and m.get("event_id") == event_id)
        sends: list[dict] = []
        while True:
            m = await self.recv(lambda m: m.get("event_id") == event_id and m.get("op") in ("send", "done"))
            if m["op"] == "done":
                # A plugin cancelled while the node delivers its reply reports done first.
                late = [b for b in self.lines.backlog if b.get("op") == "send" and b.get("event_id") == event_id]
                for b in late:
                    self.lines.backlog.remove(b)
                sends.extend(late)
                return {"event_id": event_id, "matched": matched["matched"], "sends": sends, "done": m}
            sends.append(m)

    async def llm_hook(self, event_id: str, request: dict) -> dict:
        """Runs the ``llm_request`` hooks of a default chat reply. Empty image lists are left out."""
        m = await self.hook("llm_request", event_id, request)
        out = dict(m.get("payload") or request)
        if not out.get("image_urls"):
            out.pop("image_urls", None)
        return out

    async def wait_log(self, needle: str, timeout: float = TIMEOUT) -> dict:
        for m in self.logs:
            if needle in m.get("message", ""):
                return m
        return await self.recv(lambda m: m.get("op") == "log" and needle in m.get("message", ""), timeout)


def make_event(
    text: str,
    group: bool = False,
    admin: bool = False,
    wake: bool = True,
    platform: str = "qq_official",
    user_id: str = "u1",
    nickname: str = "Alice",
    chain: list | None = None,
) -> dict[str, Any]:
    session = "g1" if group else user_id
    kind = "GroupMessage" if group else "FriendMessage"
    event: dict[str, Any] = {
        "platform": platform,
        "bot_id": "bot1",
        "self_id": "self1",
        "message_type": "group" if group else "private",
        "session_id": session,
        "unified_msg_origin": f"{platform}:{kind}:{session}",
        "sender": {"user_id": user_id, "nickname": nickname},
        "message_str": text,
        "chain": chain if chain is not None else [{"type": "plain", "text": text}],
        "is_at_or_wake": wake,
        "is_admin": admin,
        "timestamp": int(time.time()),
    }
    if group:
        event["group_id"] = "g1"
    return event


def chain_text(send: dict) -> str:
    return "".join(c.get("text", "") for c in send["chain"] if c.get("type") == "plain")


class FakeNode:
    def __init__(self, server: ForkServerProcess) -> None:
        self.server = server
        self.control: Control | None = None
        self.tmp = short_tmpdir()
        self.servers: list[asyncio.AbstractServer] = []
        self.procs: list[PluginProc] = []

    async def __aenter__(self) -> FakeNode:
        self.control = await Control.connect(self.server.socket)
        return self

    async def __aexit__(self, *exc: Any) -> None:
        assert self.control is not None
        for proc in self.procs:
            await self.control.write({"op": "kill", "proc_id": proc.proc_id})
        for proc in self.procs:
            try:
                await self.control.lines.next(
                    lambda m, p=proc: m.get("op") == "exited" and m.get("proc_id") == p.proc_id, 10
                )
            except (TimeoutError, EOFError):
                pass
        await self.control.close()
        for srv in self.servers:
            srv.close()
        shutil.rmtree(self.tmp, ignore_errors=True)

    async def spawn_raw(
        self,
        plugin: str,
        config: dict | None = None,
        plugin_dir: str | None = None,
        *,
        wait_for_memory_limit: bool = False,
        share_key: str | None = None,
        data_dir: str | None = None,
    ) -> tuple[str, dict, asyncio.Future]:
        """Sends spawn and returns the reply plus a future for the child connection."""
        assert self.control is not None
        proc_id = uuid.uuid4().hex[:8]
        sock = os.path.join(self.tmp, f"{proc_id}.sock")
        conn: asyncio.Future = asyncio.get_running_loop().create_future()

        async def on_conn(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
            if not conn.done():
                conn.set_result((reader, writer))

        srv = await asyncio.start_unix_server(on_conn, sock, limit=64 * 1024 * 1024)
        self.servers.append(srv)
        data_dir = data_dir or os.path.join(self.tmp, proc_id, "data")
        spec: dict[str, Any] = {
            "op": "spawn",
            "proc_id": proc_id,
            "plugin_dir": plugin_dir or str(PLUGINS_DIR / plugin),
            "venv_site_packages": None,
            "data_dir": data_dir,
            "plugin_name": plugin,
            "bot_id": "bot1",
            "socket": sock,
            "wait_for_memory_limit": wait_for_memory_limit,
        }
        if share_key is None:
            spec["config"] = config or {}
        else:
            spec["share_key"] = share_key
            spec["config_on_socket"] = True
        await self.control.write(spec)
        reply = await self.control.lines.next(
            lambda m: m.get("proc_id") == proc_id and m.get("op") in ("spawned", "error")
        )
        return proc_id, reply, conn

    async def group(self) -> GroupConn:
        """Starts a group process and waits for its ready message."""
        assert self.control is not None
        proc_id = uuid.uuid4().hex[:8]
        sock = os.path.join(self.tmp, f"{proc_id}.sock")
        workdir = os.path.join(self.tmp, proc_id, "work")
        os.makedirs(workdir)
        conn: asyncio.Future = asyncio.get_running_loop().create_future()

        async def on_conn(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
            if not conn.done():
                conn.set_result((reader, writer))

        srv = await asyncio.start_unix_server(on_conn, sock, limit=64 * 1024 * 1024)
        self.servers.append(srv)
        await self.control.write(
            {
                "op": "spawn",
                "mode": "group",
                "proc_id": proc_id,
                "plugin_dir": workdir,
                "data_dir": workdir,
                "plugin_name": "group",
                "bot_id": "bot1",
                "socket": sock,
                "wait_for_memory_limit": False,
            }
        )
        reply = await self.control.lines.next(
            lambda m: m.get("proc_id") == proc_id and m.get("op") in ("spawned", "error")
        )
        assert reply["op"] == "spawned", reply
        reader, writer = await asyncio.wait_for(conn, TIMEOUT)
        lines = Lines(reader)
        ready = await lines.next(lambda m: m.get("op") == "ready")
        group = GroupConn(self, proc_id, reply["pid"], lines, writer)
        self.procs.append(PluginProc(proc_id, reply["pid"], lines, writer, ready, default_llm))
        return group

    async def spawn(
        self,
        plugin: str,
        config: dict | None = None,
        llm: Callable[[dict], dict] = default_llm,
        *,
        share_key: str | None = None,
        plugin_dir: str | None = None,
        data_dir: str | None = None,
    ) -> PluginProc:
        proc_id, reply, conn = await self.spawn_raw(plugin, config, plugin_dir, share_key=share_key, data_dir=data_dir)
        assert reply["op"] == "spawned", reply
        reader, writer = await asyncio.wait_for(conn, TIMEOUT)
        if share_key is not None:
            writer.write(json.dumps({"op": "config", "config": config or {}}).encode() + b"\n")
            await writer.drain()
        lines = Lines(reader)
        logs = []
        while True:
            m = await lines.next()
            if m.get("op") == "hello":
                break
            if m.get("op") == "log":
                logs.append(m)
                if m.get("level") == "error":
                    raise RuntimeError(f"plugin failed to load: {m['message']}")
        proc = PluginProc(proc_id, reply["pid"], lines, writer, m, llm)
        proc.logs.extend(logs)
        self.procs.append(proc)
        return proc


class GroupConn:
    """Node side of one group process control socket."""

    def __init__(self, node: FakeNode, proc_id: str, pid: int, lines: Lines, writer: asyncio.StreamWriter) -> None:
        self.node = node
        self.proc_id = proc_id
        self.pid = pid
        self.lines = lines
        self.writer = writer

    async def write(self, message: dict) -> None:
        self.writer.write(json.dumps(message).encode() + b"\n")
        await self.writer.drain()

    async def load_raw(
        self,
        plugin: str,
        config: dict | None = None,
        plugin_dir: str | None = None,
        data_dir: str | None = None,
        venv: str | None = None,
    ) -> tuple[str, Lines, asyncio.StreamWriter]:
        """Sends load and the config line. Returns the slot proc_id and its socket."""
        proc_id = uuid.uuid4().hex[:8]
        sock = os.path.join(self.node.tmp, f"{proc_id}.sock")
        conn: asyncio.Future = asyncio.get_running_loop().create_future()

        async def on_conn(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
            if not conn.done():
                conn.set_result((reader, writer))

        srv = await asyncio.start_unix_server(on_conn, sock, limit=64 * 1024 * 1024)
        self.node.servers.append(srv)
        await self.write(
            {
                "op": "load",
                "proc_id": proc_id,
                "plugin_dir": plugin_dir or str(PLUGINS_DIR / plugin),
                "plugin_name": plugin,
                "data_dir": data_dir or os.path.join(self.node.tmp, proc_id, "data"),
                "venv_site_packages": venv,
                "socket": sock,
            }
        )
        reader, writer = await asyncio.wait_for(conn, TIMEOUT)
        writer.write(json.dumps({"op": "config", "config": config or {}}).encode() + b"\n")
        await writer.drain()
        return proc_id, Lines(reader), writer

    async def load(
        self, plugin: str, config: dict | None = None, llm: Callable[[dict], dict] = default_llm, **kw: Any
    ) -> PluginProc:
        proc_id, lines, writer = await self.load_raw(plugin, config, **kw)
        logs = []
        while True:
            m = await lines.next()
            if m.get("op") == "hello":
                break
            if m.get("op") == "log":
                logs.append(m)
        proc = PluginProc(proc_id, self.pid, lines, writer, m, llm)
        proc.logs.extend(logs)
        return proc

    async def exited(self, proc_id: str, timeout: float = TIMEOUT) -> dict:
        return await self.lines.next(lambda m: m.get("op") == "slot_exited" and m.get("proc_id") == proc_id, timeout)


@pytest.fixture(scope="session")
def fork_server():
    server = ForkServerProcess()
    yield server
    server.stop()


@pytest.fixture
def run():
    def runner(coro):
        return asyncio.run(asyncio.wait_for(coro, 60))

    return runner
