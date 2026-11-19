"""Fork server. Preloads heavy modules once, then forks one process per plugin.

Run as ``python -m fish_sidecar.server --socket <path>``. The server listens on the
socket and serves one node connection at a time. A new connection replaces the old one.
"""

from __future__ import annotations

import argparse
import errno
import gc
import importlib
import logging
import os
import selectors
import signal
import socket
import sys
import time
import traceback
from collections import deque
from typing import Any

from fish_sidecar import protocol

log = logging.getLogger("fish_sidecar")

PRELOAD = (
    "asyncio",
    "json",
    "aiohttp",
    "httpx",
    "pydantic",
    "yaml",
    "astrbot",
    "astrbot.api",
    "astrbot.api.all",
    "astrbot.api.event",
    "astrbot.api.event.filter",
    "astrbot.api.star",
    "astrbot.api.message_components",
    "astrbot.api.platform",
    "astrbot.api.provider",
    "astrbot.core.utils.session_waiter",
    "fish_sidecar.loader",
    "fish_sidecar.dispatch",
    "fish_sidecar.child",
)
KILL_GRACE_S = 5.0
SHUTDOWN_GRACE_S = 5.0
MAX_QUEUED = 4096


def preload() -> list[str]:
    """Imports shared modules so forked children inherit them. Returns modules that failed."""
    failed = []
    for name in PRELOAD:
        try:
            importlib.import_module(name)
        except Exception as e:
            failed.append(f"{name}: {e}")
    gc.collect()
    gc.freeze()
    return failed


class _Child:
    __slots__ = ("proc_id", "pid", "kill_deadline")

    def __init__(self, proc_id: str, pid: int) -> None:
        self.proc_id = proc_id
        self.pid = pid
        self.kill_deadline: float | None = None


class ForkServer:
    def __init__(self, socket_path: str) -> None:
        self.socket_path = socket_path
        self.sel = selectors.DefaultSelector()
        self.listener: socket.socket | None = None
        self.conn: socket.socket | None = None
        self.inbuf = bytearray()
        self.outbuf = bytearray()
        self.queued: deque[dict[str, Any]] = deque(maxlen=MAX_QUEUED)
        self.children: dict[int, _Child] = {}
        self.by_proc: dict[str, _Child] = {}
        self.stopping = False
        self.ppid = os.getppid()
        self.wake_r, self.wake_w = os.pipe()

    def listen(self) -> None:
        try:
            os.unlink(self.socket_path)
        except FileNotFoundError:
            pass
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(self.socket_path)
        os.chmod(self.socket_path, 0o600)
        listener.listen(4)
        listener.setblocking(False)
        self.listener = listener
        self.sel.register(listener, selectors.EVENT_READ, self.on_accept)
        os.set_blocking(self.wake_r, False)
        os.set_blocking(self.wake_w, False)
        signal.set_wakeup_fd(self.wake_w, warn_on_full_buffer=False)
        self.sel.register(self.wake_r, selectors.EVENT_READ, self.on_wake)
        for sig in (signal.SIGCHLD, signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(sig, self.on_signal)

    def on_signal(self, signum: int, _frame: Any) -> None:
        if signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            self.stopping = True

    def on_wake(self, _key: Any) -> None:
        try:
            while os.read(self.wake_r, 512):
                pass
        except BlockingIOError:
            pass

    def on_accept(self, _key: Any) -> None:
        assert self.listener is not None
        try:
            conn, _ = self.listener.accept()
        except BlockingIOError:
            return
        if self.conn is not None:
            self.drop_conn()
        conn.setblocking(False)
        self.conn = conn
        self.inbuf.clear()
        self.outbuf.clear()
        self.sel.register(conn, selectors.EVENT_READ, self.on_conn)
        while self.queued:
            self.send(self.queued.popleft())

    def drop_conn(self) -> None:
        if self.conn is None:
            return
        try:
            self.sel.unregister(self.conn)
        except (KeyError, ValueError):
            pass
        self.conn.close()
        self.conn = None
        self.inbuf.clear()
        self.outbuf.clear()

    def on_conn(self, key: selectors.SelectorKey) -> None:
        if key.events & selectors.EVENT_WRITE or self.outbuf:
            self.flush()
        if self.conn is None:
            return
        try:
            data = self.conn.recv(65536)
        except BlockingIOError:
            return
        except OSError:
            self.drop_conn()
            return
        if not data:
            self.drop_conn()
            return
        self.inbuf += data
        if len(self.inbuf) > protocol.MAX_LINE:
            log.error("control line too long, dropping connection")
            self.drop_conn()
            return
        while True:
            nl = self.inbuf.find(b"\n")
            if nl < 0:
                break
            line = bytes(self.inbuf[:nl])
            del self.inbuf[: nl + 1]
            try:
                message = protocol.decode(line)
            except ValueError:
                log.warning("invalid control line ignored")
                continue
            if message is not None:
                self.handle(message)

    def send(self, message: dict[str, Any]) -> None:
        if self.conn is None:
            self.queued.append(message)
            return
        self.outbuf += protocol.encode(message)
        self.flush()

    def flush(self) -> None:
        if self.conn is None:
            return
        while self.outbuf:
            try:
                n = self.conn.send(self.outbuf)
            except BlockingIOError:
                break
            except OSError:
                self.drop_conn()
                return
            del self.outbuf[:n]
        events = selectors.EVENT_READ | (selectors.EVENT_WRITE if self.outbuf else 0)
        self.sel.modify(self.conn, events, self.on_conn)

    def handle(self, message: dict[str, Any]) -> None:
        op = message.get("op")
        if op == "spawn":
            self.spawn(message)
        elif op == "kill":
            self.kill(str(message.get("proc_id") or ""))
        else:
            log.warning("unknown control op %r", op)

    def spawn(self, spec: dict[str, Any]) -> None:
        proc_id = str(spec.get("proc_id") or "")
        missing = [k for k in ("proc_id", "plugin_dir", "socket") if not spec.get(k)]
        if missing:
            self.send({"op": "error", "proc_id": proc_id, "message": f"spawn is missing {', '.join(missing)}"})
            return
        if proc_id in self.by_proc:
            self.send({"op": "error", "proc_id": proc_id, "message": "proc_id is already running"})
            return
        if not os.path.isdir(str(spec["plugin_dir"])):
            self.send({"op": "error", "proc_id": proc_id, "message": f"plugin_dir {spec['plugin_dir']} does not exist"})
            return
        sys.stdout.flush()
        sys.stderr.flush()
        try:
            pid = os.fork()
        except OSError as e:
            self.send({"op": "error", "proc_id": proc_id, "message": f"fork failed: {e}"})
            return
        if pid == 0:
            self.become_child(spec)
        child = _Child(proc_id, pid)
        self.children[pid] = child
        self.by_proc[proc_id] = child
        self.send({"op": "spawned", "proc_id": proc_id, "pid": pid})

    def become_child(self, spec: dict[str, Any]) -> None:
        code = 1
        try:
            signal.set_wakeup_fd(-1)
            for sig in (signal.SIGCHLD, signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
                signal.signal(sig, signal.SIG_DFL)
            self.sel.close()
            for fd in (self.wake_r, self.wake_w):
                os.close(fd)
            if self.listener is not None:
                self.listener.close()
            if self.conn is not None:
                self.conn.close()
            os.setsid()
            gc.unfreeze()
            from fish_sidecar import child

            code = child.run(spec)
        except SystemExit as e:
            code = e.code if isinstance(e.code, int) else 1
        except BaseException:
            try:
                traceback.print_exc(file=sys.__stderr__)
            except Exception:
                pass
            code = 1
        finally:
            try:
                sys.__stdout__.flush()
                sys.__stderr__.flush()
            except Exception:
                pass
            os._exit(code)

    def kill(self, proc_id: str) -> None:
        child = self.by_proc.get(proc_id)
        if child is None:
            return
        self.signal_child(child, signal.SIGTERM)
        if child.kill_deadline is None:
            child.kill_deadline = time.monotonic() + KILL_GRACE_S

    def signal_child(self, child: _Child, sig: int) -> None:
        try:
            os.killpg(child.pid, sig)
        except ProcessLookupError:
            pass
        except PermissionError:
            try:
                os.kill(child.pid, sig)
            except ProcessLookupError:
                pass

    def reap(self) -> None:
        while self.children:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return
            except InterruptedError:
                continue
            if pid == 0:
                return
            child = self.children.pop(pid, None)
            if child is None:
                continue
            self.by_proc.pop(child.proc_id, None)
            if os.WIFSIGNALED(status):
                code = -os.WTERMSIG(status)
            else:
                code = os.waitstatus_to_exitcode(status)
            try:
                os.killpg(pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
            self.send({"op": "exited", "proc_id": child.proc_id, "pid": pid, "code": code})

    def enforce_kill_deadlines(self) -> float | None:
        now = time.monotonic()
        nearest: float | None = None
        for child in list(self.children.values()):
            if child.kill_deadline is None:
                continue
            if now >= child.kill_deadline:
                self.signal_child(child, signal.SIGKILL)
                child.kill_deadline = now + 60
            left = child.kill_deadline - now
            nearest = left if nearest is None else min(nearest, left)
        return nearest

    def serve(self) -> None:
        while not self.stopping:
            timeout = self.enforce_kill_deadlines()
            timeout = 1.0 if timeout is None else max(0.0, min(timeout, 1.0))
            try:
                events = self.sel.select(timeout)
            except InterruptedError:
                events = []
            for key, _mask in events:
                key.data(key)
            self.reap()
            if os.getppid() != self.ppid:
                log.warning("parent process changed, shutting down")
                self.stopping = True

    def shutdown(self) -> None:
        for child in list(self.children.values()):
            self.signal_child(child, signal.SIGTERM)
        deadline = time.monotonic() + SHUTDOWN_GRACE_S
        while self.children and time.monotonic() < deadline:
            self.reap()
            if self.children:
                time.sleep(0.05)
        for child in list(self.children.values()):
            self.signal_child(child, signal.SIGKILL)
        end = time.monotonic() + 1.0
        while self.children and time.monotonic() < end:
            self.reap()
            if self.children:
                time.sleep(0.02)
        if self.conn is not None and self.outbuf:
            try:
                self.conn.setblocking(True)
                self.conn.sendall(self.outbuf)
            except OSError:
                pass
        self.drop_conn()
        if self.listener is not None:
            self.listener.close()
        try:
            os.unlink(self.socket_path)
        except OSError as e:
            if e.errno != errno.ENOENT:
                raise
        signal.set_wakeup_fd(-1)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="fish_sidecar.server")
    parser.add_argument("--socket", required=True, help="unix socket path to listen on")
    parser.add_argument("--log-level", default=os.environ.get("FISH_SIDECAR_LOG", "info"))
    args = parser.parse_args(argv)
    logging.basicConfig(
        level=getattr(logging, str(args.log_level).upper(), logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        stream=sys.stderr,
    )
    for failure in preload():
        log.warning("preload failed: %s", failure)
    server = ForkServer(args.socket)
    server.listen()
    log.info("fork server listening on %s", args.socket)
    try:
        server.serve()
    finally:
        server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
