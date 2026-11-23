"""Plugin process: connects to the node, loads one plugin and serves events."""

from __future__ import annotations

import asyncio
import io
import logging
import signal
import sys
import threading
import traceback
import uuid
from pathlib import Path
from typing import Any

from astrbot.core.star.star_handler import EventType

from fish_sidecar import astrbot_boot, loader, protocol, runtime
from fish_sidecar.adapter import LLMError
from fish_sidecar.dispatch import EVENT_HOOK_KINDS, Dispatcher, describe_error

log = logging.getLogger("astrbot")

EXIT_OK = 0
EXIT_LOAD_FAILED = 2
EXIT_CONNECT_FAILED = 3
EXIT_UNGROUPABLE = 4
LLM_TIMEOUT_S = 180
TERMINATE_TIMEOUT_S = 5


class _LineWriter(io.TextIOBase):
    """File object that turns each written line into a log op."""

    def __init__(self, runtime: ChildRuntime, level: str) -> None:
        self.runtime = runtime
        self.level = level
        self._buf = ""
        self._lock = threading.Lock()

    def writable(self) -> bool:
        return True

    def write(self, s: str) -> int:
        with self._lock:
            self._buf += s
            *lines, self._buf = self._buf.split("\n")
        for line in lines:
            if line.strip():
                self.runtime.write_threadsafe({"op": "log", "level": self.level, "message": line})
        return len(s)

    def flush(self) -> None:
        with self._lock:
            line, self._buf = self._buf, ""
        if line.strip():
            self.runtime.write_threadsafe({"op": "log", "level": self.level, "message": line})


class Ungroupable(Exception):
    """The plugin cannot share a process with other plugins. The message says why."""


def _idle_thread(thread: threading.Thread) -> bool:
    """True for threads that exist while the plugin is idle: asyncio executor workers, AstrBot's SQLite connection workers and fish's own threads."""
    if thread.name.startswith("asyncio") or getattr(thread, "fish_internal", False):
        return True
    target = getattr(thread, "_target", None)
    return (getattr(target, "__module__", None) or "").startswith("aiosqlite")


class ChildRuntime:
    """Socket side of the plugin process. Registered with ``fish_sidecar.runtime``."""

    def __init__(self, spec: dict[str, Any]) -> None:
        self.spec = spec
        self.proc_id = str(spec.get("proc_id") or "")
        self.plugin_name = str(spec.get("plugin_name") or "plugin")
        self.plugin_dir = Path(spec["plugin_dir"]).resolve()
        self.data_dir = Path(spec.get("data_dir") or self.plugin_dir / "data").resolve()
        self.loop = asyncio.get_running_loop()
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None
        self.pending: dict[str, asyncio.Future] = {}
        self.tasks: set[asyncio.Task] = set()
        self.queued_tasks: list[tuple[Any, str]] = []
        self.ready = False
        self.stopping = asyncio.Event()
        self.dispatcher: Dispatcher | None = None
        self.wake_commands: list[str] | None = None
        self.plugin: loader.LoadedPlugin | None = None
        self.main_task: asyncio.Task | None = None
        self.reader_task: asyncio.Task | None = None

    def write(self, message: dict[str, Any]) -> None:
        if self.writer is None or self.writer.is_closing():
            return
        self.writer.write(protocol.encode(message))

    def write_threadsafe(self, message: dict[str, Any]) -> None:
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is self.loop:
            self.write(message)
        elif not self.loop.is_closed():
            self.loop.call_soon_threadsafe(self.write, message)

    async def send(self, event_id: str | None, session: str, chain: list[dict]) -> None:
        message: dict[str, Any] = {"op": "send", "session": session, "chain": chain}
        if event_id:
            message["event_id"] = event_id
        self.write(message)
        await self._drain()

    async def send_to(self, session: str, chain: list[dict]) -> None:
        self.write({"op": "send_to", "session": session, "chain": chain})
        await self._drain()

    async def llm(
        self,
        prompt: str | None,
        system_prompt: str | None,
        contexts: list[dict],
        image_urls: list[str],
        tools: list[dict],
    ) -> dict[str, Any]:
        message: dict[str, Any] = {"op": "llm"}
        if prompt is not None:
            message["prompt"] = prompt
        if system_prompt:
            message["system_prompt"] = system_prompt
        if contexts:
            message["contexts"] = contexts
        if image_urls:
            message["image_urls"] = image_urls
        if tools:
            message["tools"] = tools
        reply = await self.call_node(message, LLM_TIMEOUT_S)
        if reply.get("error"):
            raise LLMError(str(reply["error"]))
        return reply

    async def call_node(self, message: dict[str, Any], timeout: float) -> dict[str, Any]:
        """Writes ``message`` with a new ``req_id`` and returns the node reply that carries the same ``req_id``."""
        req_id = uuid.uuid4().hex
        future = self.loop.create_future()
        self.pending[req_id] = future
        self.write({**message, "req_id": req_id})
        try:
            return await asyncio.wait_for(future, timeout)
        finally:
            self.pending.pop(req_id, None)

    def start_task(self, awaitable: Any, desc: str) -> None:
        if not self.ready:
            self.queued_tasks.append((awaitable, desc))
            return
        self.spawn(self._guard(awaitable, desc))

    async def _guard(self, awaitable: Any, desc: str) -> None:
        try:
            await awaitable
        except asyncio.CancelledError:
            raise
        except Exception as e:
            log.error("task %s failed: %s\n%s", desc, describe_error(e), traceback.format_exc(limit=8))

    def spawn(self, coro: Any) -> asyncio.Task:
        task = self.loop.create_task(coro)
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)
        return task

    async def _drain(self) -> None:
        if self.writer is not None and not self.writer.is_closing():
            try:
                await self.writer.drain()
            except ConnectionError:
                self.stopping.set()

    async def connect(self) -> None:
        path = str(self.spec["socket"])
        last: Exception | None = None
        for _ in range(50):
            try:
                self.reader, self.writer = await asyncio.open_unix_connection(path, limit=protocol.MAX_LINE)
                return
            except OSError as e:
                last = e
                await asyncio.sleep(0.1)
        raise ConnectionError(f"cannot connect to {path}: {last}")

    def attach(self) -> None:
        """Makes this runtime the target of adapter calls, logs and SIGTERM."""
        runtime.set_runtime(self)
        self.install_logging()
        self.loop.add_signal_handler(signal.SIGTERM, self.stopping.set)

    def import_plugin(self) -> tuple[str, Any]:
        """Imports the plugin package and returns its name and ``main`` module."""
        preimported = None
        if self.spec.get("preimported"):
            from fish_sidecar import template

            preimported = template.preimported
            if preimported is None:
                raise RuntimeError("template did not import the plugin")
        return loader.import_main(
            self.plugin_dir,
            self.spec.get("venv_site_packages"),
            self.data_dir,
            self.plugin_name,
            preimported,
        )

    def module_prefix(self, package: str) -> str | None:
        """Package that owns the handlers this runtime serves. None serves every handler."""
        return None

    def install_logging(self) -> None:
        """Sends AstrBot, plugin and standard library log records, and stdout and stderr lines, to the node."""
        debug = bool(self.spec.get("debug"))

        def sink(level: str, message: str) -> None:
            if level != "debug" or debug:
                self.write_threadsafe({"op": "log", "level": level, "message": message})

        astrbot_boot.route_logs(sink)
        sys.stdout = _LineWriter(self, "info")
        sys.stderr = _LineWriter(self, "warning")

    async def load(self) -> None:
        # The import runs on the event loop thread, so module level code can create tasks and schedulers.
        package, main = self.import_plugin()
        moved = await astrbot_boot.migrate_legacy_kv(str(self.data_dir))
        if moved:
            self.write(
                {
                    "op": "log",
                    "level": "info",
                    "message": f"已把 {moved} 条插件数据从 {astrbot_boot.LEGACY_KV} 迁入 AstrBot 数据库",
                }
            )
        md = loader.find_star(package, main)
        md.module = main
        md.root_dir_name = self.plugin_dir.name
        self.plugin = await loader.instantiate(md, self.plugin_dir, self.plugin_name, self.spec.get("config") or {})
        self.plugin.package = package
        self.dispatcher = Dispatcher(self, self.plugin.instance, self.plugin.name, self.module_prefix(package))
        await self.plugin.context.kb_manager.initialize()
        await self.plugin.instance.initialize()
        await self.plugin.context.cron_manager.start(self.plugin.context)

    def hello(self) -> None:
        assert self.plugin is not None and self.dispatcher is not None
        if self.plugin.command_only_idle:
            self.wake_commands = self.dispatcher.wake_commands()
        handlers = self.dispatcher.descriptors()
        # Hooks and tools run after the event was handled, so they need the event kept.
        self.dispatcher.keep_events = any(h["kind"] in EVENT_HOOK_KINDS or h["kind"] == "llm_tool" for h in handlers)
        self.write(
            {
                "op": "hello",
                "proc_id": self.proc_id,
                "plugin": {
                    "name": self.plugin.name,
                    "version": self.plugin.version,
                    "handlers": handlers,
                    "wake_commands": self.wake_commands,
                },
            }
        )
        inert = self.dispatcher.inert_handlers()
        if inert:
            log.warning("these handlers are registered but never called in fish: %s", ", ".join(inert))

    async def read_loop(self) -> None:
        assert self.reader is not None
        while not self.stopping.is_set():
            try:
                line = await self.reader.readline()
            except (ConnectionError, ValueError) as e:
                log.error("socket read failed: %s", e)
                break
            if not line:
                break
            try:
                message = protocol.decode(line)
            except ValueError:
                continue
            if message is not None:
                self.on_message(message)
        self.stopping.set()

    def on_message(self, message: dict[str, Any]) -> None:
        op = message.get("op")
        if op in ("llm_result", "result_done", "agent_result"):
            future = self.pending.get(str(message.get("req_id")))
            if future is not None and not future.done():
                future.set_result(message)
        elif op == "event" and self.dispatcher is not None:
            self.spawn(self.dispatcher.handle_event(str(message.get("event_id")), message.get("event") or {}))
        elif op == "hook" and self.dispatcher is not None:
            self.spawn(self._hook(message))
        elif op == "tool_call" and self.dispatcher is not None:
            self.spawn(self._tool_call(message))
        elif op == "config" and self.plugin is not None and self.plugin.config is not None:
            loader.replace_config(self.plugin.config, message.get("config") or {})
        elif op == "idle_check":
            self.write({"op": "idle_result", "busy": self.busy_reasons()})
        elif op == "shutdown":
            self.stopping.set()

    def owns_context(self, ctx: Any) -> bool:
        """True when work running in the contextvars context ``ctx`` belongs to this plugin."""
        return True

    def own_fds(self) -> set[int]:
        """Descriptors of this runtime itself, which do not count as plugin connections."""
        fds = {self.loop._ssock.fileno()}  # type: ignore[attr-defined]
        sock = self.writer.get_extra_info("socket") if self.writer is not None else None
        if sock is not None:
            fds.add(sock.fileno())
        return fds

    def busy_reasons(self) -> list[str]:
        """Describes work that would be lost if the process stopped now. Empty when the plugin is idle."""
        reasons: list[str] = []
        own = {self.main_task, self.reader_task, asyncio.current_task()}
        tasks = [
            t
            for t in asyncio.all_tasks(self.loop)
            if t not in own and not t.done() and self.owns_context(t.get_context())
        ]
        if tasks:
            reasons.append(f"{len(tasks)} 个后台任务")
        if self.pending:
            reasons.append(f"{len(self.pending)} 个进行中的模型请求")
        if self.wake_commands is not None and self.dispatcher is not None:
            if self.dispatcher.wake_commands() != self.wake_commands:
                reasons.append("指令唤醒条件已变化")
        try:
            timers = [
                h
                for h in self.loop._scheduled  # type: ignore[attr-defined]
                if not h.cancelled() and self.owns_context(getattr(h, "_context", None))
            ]
            if timers:
                reasons.append(f"{len(timers)} 个定时器")
            own_fds = self.own_fds()
            fds = [k.fd for k in self.loop._selector.get_map().values() if k.fd not in own_fds]  # type: ignore[attr-defined]
            if fds:
                reasons.append(f"{len(fds)} 个打开的网络连接")
        except Exception as e:
            reasons.append(f"无法检查事件循环: {e}")
        threads = [
            t.name
            for t in threading.enumerate()
            if t is not threading.main_thread() and t.is_alive() and not _idle_thread(t)
        ]
        if threads:
            reasons.append("线程 " + ", ".join(sorted(threads)[:3]))
        return reasons

    async def _hook(self, message: dict[str, Any]) -> None:
        assert self.dispatcher is not None
        kind = str(message.get("kind"))
        try:
            result = await self.dispatcher.handle_hook(
                kind, message.get("event_id"), message.get("event"), message.get("payload") or {}
            )
        except Exception as e:
            log.error("%s hook failed: %s\n%s", kind, describe_error(e), traceback.format_exc(limit=8))
            result = {"stopped": False}
        self.write({"op": "hook_result", "hook_id": message.get("hook_id"), **result})

    async def _tool_call(self, message: dict[str, Any]) -> None:
        assert self.dispatcher is not None
        call_id = str(message.get("call_id"))
        try:
            result = await self.dispatcher.handle_tool_call(
                str(message.get("event_id")),
                message.get("event") or {},
                str(message.get("name")),
                message.get("arguments"),
            )
        except Exception as e:
            log.error("tool call failed: %s", describe_error(e))
            result = {"error": describe_error(e)}
        self.write({"op": "tool_result", "call_id": call_id, **result})

    async def run(self) -> int:
        try:
            await self.connect()
            assert self.reader is not None
            if self.spec.get("wait_for_memory_limit"):
                # The node attaches this process to the tenant cgroup before allowing plugin imports.
                if await asyncio.wait_for(self.reader.readexactly(1), timeout=60) != b"\x01":
                    raise ConnectionError("memory limit was not confirmed")
            if self.spec.get("config_on_socket"):
                # The plugin config arrives on this socket instead of through the fork server.
                line = await asyncio.wait_for(self.reader.readline(), timeout=60)
                message = protocol.decode(line) if line else None
                if not message or message.get("op") != "config":
                    raise ConnectionError("plugin config was not received")
                self.spec["config"] = message.get("config") or {}
        except (ConnectionError, TimeoutError, ValueError, asyncio.IncompleteReadError) as e:
            print(f"fish child {self.proc_id}: {e}", file=sys.__stderr__)
            return EXIT_CONNECT_FAILED
        self.attach()
        if self.spec.get("preimported"):
            from fish_sidecar import template

            for level, line in template.import_logs:
                self.write({"op": "log", "level": level, "message": line})
        note = self.spec.get("template_note")
        if note:
            self.write({"op": "log", "level": "info", "message": f"插件单独加载，未与其他机器人共享导入: {note}"})
        self.main_task = asyncio.current_task()
        reader_task = self.reader_task = self.loop.create_task(self.read_loop())
        try:
            await self.load()
        except Ungroupable as e:
            self.write({"op": "log", "level": "info", "message": f"插件改为单独运行: {e}"})
            await self._drain()
            reader_task.cancel()
            return EXIT_UNGROUPABLE
        except BaseException as e:
            self.write({"op": "log", "level": "error", "message": f"plugin load failed: {describe_error(e)}"})
            self.write({"op": "log", "level": "debug", "message": traceback.format_exc(limit=12)})
            await self._drain()
            reader_task.cancel()
            return EXIT_LOAD_FAILED
        self.hello()
        self.ready = True
        for awaitable, desc in self.queued_tasks:
            self.spawn(self._guard(awaitable, desc))
        self.queued_tasks.clear()
        assert self.dispatcher is not None
        await self.dispatcher.call_hooks(EventType.OnAstrBotLoadedEvent)
        await self.dispatcher.call_hooks(EventType.OnPlatformLoadedEvent)
        await self._drain()
        await self.stopping.wait()
        await self.shutdown()
        reader_task.cancel()
        return EXIT_OK

    async def shutdown(self) -> None:
        if self.plugin is not None:
            await self.plugin.context.cron_manager.shutdown()
            try:
                await asyncio.wait_for(self.plugin.instance.terminate(), TERMINATE_TIMEOUT_S)
            except Exception as e:
                log.warning("terminate failed: %s", describe_error(e))
        for task in list(self.tasks):
            task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)
        if self.plugin is not None:
            await self.plugin.context.kb_manager.terminate()
        await self._drain()
        if self.writer is not None:
            self.writer.close()


async def _main(spec: dict[str, Any]) -> int:
    return await ChildRuntime(spec).run()


def run(spec: dict[str, Any]) -> int:
    """Entry point inside a forked process. Returns the exit code."""
    return asyncio.run(_main(spec))
