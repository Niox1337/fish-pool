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

from astrbot import _fish
from astrbot.core.provider.provider import LLMError
from astrbot.core.star.star_handler import EventType
from fish_sidecar import loader, protocol
from fish_sidecar.dispatch import Dispatcher, describe_error

log = logging.getLogger("astrbot")

EXIT_OK = 0
EXIT_LOAD_FAILED = 2
EXIT_CONNECT_FAILED = 3
LLM_TIMEOUT_S = 180
TERMINATE_TIMEOUT_S = 5

_LEVELS = {logging.DEBUG: "debug", logging.INFO: "info", logging.WARNING: "warning", logging.ERROR: "error", logging.CRITICAL: "error"}


class _LogHandler(logging.Handler):
    def __init__(self, runtime: ChildRuntime) -> None:
        super().__init__(logging.DEBUG)
        self.runtime = runtime
        self.setFormatter(logging.Formatter("%(message)s"))

    def emit(self, record: logging.LogRecord) -> None:
        try:
            message = self.format(record)
            level = _LEVELS.get(record.levelno, "info")
            self.runtime.write_threadsafe({"op": "log", "level": level, "message": message})
        except Exception:
            pass


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


class ChildRuntime:
    """Socket side of the plugin process. Registered with ``astrbot._fish``."""

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

    async def llm(self, prompt: str | None, system_prompt: str | None, contexts: list[dict] | None, image_urls: list[str] | None) -> str:
        req_id = uuid.uuid4().hex
        message: dict[str, Any] = {"op": "llm", "req_id": req_id, "prompt": prompt or ""}
        if system_prompt:
            message["system_prompt"] = system_prompt
        if contexts:
            message["contexts"] = contexts
        if image_urls:
            message["image_urls"] = image_urls
        future = self.loop.create_future()
        self.pending[req_id] = future
        self.write(message)
        try:
            reply = await asyncio.wait_for(future, LLM_TIMEOUT_S)
        finally:
            self.pending.pop(req_id, None)
        if reply.get("error"):
            raise LLMError(str(reply["error"]))
        return str(reply.get("text") or "")

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

    def install_logging(self) -> None:
        root = logging.getLogger()
        for handler in list(root.handlers):
            root.removeHandler(handler)
        root.addHandler(_LogHandler(self))
        root.setLevel(logging.INFO)
        logging.getLogger("astrbot").setLevel(logging.DEBUG if self.spec.get("debug") else logging.INFO)
        sys.stdout = _LineWriter(self, "info")
        sys.stderr = _LineWriter(self, "warning")

    async def load(self) -> None:
        preimported = None
        if self.spec.get("preimported"):
            from fish_sidecar import template

            preimported = template.preimported
            if preimported is None:
                raise RuntimeError("template did not import the plugin")
        self.plugin = await asyncio.to_thread(
            loader.load,
            self.plugin_dir,
            self.spec.get("venv_site_packages"),
            self.data_dir,
            self.plugin_name,
            self.spec.get("config") or {},
            preimported,
        )
        self.dispatcher = Dispatcher(self, self.plugin.instance, self.plugin.name)
        await self.plugin.instance.initialize()

    def hello(self) -> None:
        assert self.plugin is not None and self.dispatcher is not None
        self.write(
            {
                "op": "hello",
                "proc_id": self.proc_id,
                "plugin": {
                    "name": self.plugin.name,
                    "version": self.plugin.version,
                    "handlers": self.dispatcher.descriptors(),
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
        if op == "llm_result":
            future = self.pending.get(str(message.get("req_id")))
            if future is not None and not future.done():
                future.set_result(message)
        elif op == "event" and self.dispatcher is not None:
            self.spawn(self.dispatcher.handle_event(str(message.get("event_id")), message.get("event") or {}))
        elif op == "llm_hook" and self.dispatcher is not None:
            self.spawn(self._llm_hook(str(message.get("event_id")), message.get("request") or {}))
        elif op == "tool_call" and self.dispatcher is not None:
            self.spawn(self._tool_call(message))
        elif op == "config" and self.plugin is not None:
            self.plugin.config.replace_values(message.get("config") or {})
        elif op == "idle_check":
            self.write({"op": "idle_result", "busy": self.busy_reasons()})
        elif op == "shutdown":
            self.stopping.set()

    def busy_reasons(self) -> list[str]:
        """Describes work that would be lost if the process stopped now. Empty when the plugin is idle."""
        reasons: list[str] = []
        own = {self.main_task, self.reader_task, asyncio.current_task()}
        tasks = [t for t in asyncio.all_tasks(self.loop) if t not in own and not t.done()]
        if tasks:
            reasons.append(f"{len(tasks)} 个后台任务")
        if self.pending:
            reasons.append(f"{len(self.pending)} 个进行中的模型请求")
        try:
            timers = [h for h in self.loop._scheduled if not h.cancelled()]  # type: ignore[attr-defined]
            if timers:
                reasons.append(f"{len(timers)} 个定时器")
            own_fds = {self.loop._ssock.fileno()}  # type: ignore[attr-defined]
            sock = self.writer.get_extra_info("socket") if self.writer is not None else None
            if sock is not None:
                own_fds.add(sock.fileno())
            fds = [k.fd for k in self.loop._selector.get_map().values() if k.fd not in own_fds]  # type: ignore[attr-defined]
            if fds:
                reasons.append(f"{len(fds)} 个打开的网络连接")
        except Exception as e:
            reasons.append(f"无法检查事件循环: {e}")
        threads = [t.name for t in threading.enumerate() if t is not threading.main_thread() and t.is_alive() and not t.name.startswith("asyncio")]
        if threads:
            reasons.append("线程 " + ", ".join(sorted(threads)[:3]))
        return reasons

    async def _llm_hook(self, event_id: str, request: dict[str, Any]) -> None:
        assert self.dispatcher is not None
        try:
            result = await self.dispatcher.handle_llm_hook(event_id, request)
        except Exception as e:
            log.error("llm hook failed: %s", describe_error(e))
            result = request
        self.write({"op": "llm_hook_result", "event_id": event_id, "request": result})

    async def _tool_call(self, message: dict[str, Any]) -> None:
        assert self.dispatcher is not None
        call_id = str(message.get("call_id"))
        try:
            result = await self.dispatcher.handle_tool_call(
                str(message.get("event_id")), message.get("event") or {}, str(message.get("name")), message.get("arguments")
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
        _fish.set_runtime(self)
        self.install_logging()
        if self.spec.get("preimported"):
            from fish_sidecar import template

            for level, line in template.import_logs:
                self.write({"op": "log", "level": level, "message": line})
        note = self.spec.get("template_note")
        if note:
            self.write({"op": "log", "level": "info", "message": f"插件单独加载，未与其他机器人共享导入: {note}"})
        self.loop.add_signal_handler(signal.SIGTERM, self.stopping.set)
        self.main_task = asyncio.current_task()
        reader_task = self.reader_task = self.loop.create_task(self.read_loop())
        try:
            await self.load()
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
            try:
                await asyncio.wait_for(self.plugin.instance.terminate(), TERMINATE_TIMEOUT_S)
            except Exception as e:
                log.warning("terminate failed: %s", describe_error(e))
        for task in list(self.tasks):
            task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)
        await self._drain()
        if self.writer is not None:
            self.writer.close()


async def _main(spec: dict[str, Any]) -> int:
    return await ChildRuntime(spec).run()


def run(spec: dict[str, Any]) -> int:
    """Entry point inside a forked process. Returns the exit code."""
    return asyncio.run(_main(spec))
