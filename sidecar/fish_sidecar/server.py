"""Fork server. Preloads heavy modules once, then forks one process per plugin.

Run as ``python -m fish_sidecar.server --socket <path>``. The server listens on the
socket and serves one node connection at a time. A new connection replaces the old one.

A spawn with ``share_key`` names a shared install of the plugin. On Linux the server keeps one
template process per shared install and plugin name (see ``fish_sidecar.template``). The template
imports the plugin once and forks each plugin process, so all of them share the imported modules.
The server is a child subreaper, so those plugin processes become its children like directly
forked ones. A plugin whose import is not safe to share is spawned directly.
"""

from __future__ import annotations

import argparse
import errno
import gc
import importlib
import itertools
import logging
import os
import selectors
import shutil
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
    "PIL.Image",
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
TEMPLATE_PREPARE_S = 90.0
TEMPLATE_IDLE_S = 300.0
TEMPLATE_SEND_TIMEOUT_S = 5.0
MAX_EARLY_EXITS = 1024
EARLY_EXIT_TTL_S = 30.0
MAX_UNSHAREABLE = 1024
PR_SET_CHILD_SUBREAPER = 36


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


def enable_subreaper() -> bool:
    """Makes orphaned descendants children of this process. Returns False when unsupported."""
    if not sys.platform.startswith("linux"):
        return False
    try:
        import ctypes

        return ctypes.CDLL(None, use_errno=True).prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0
    except Exception:
        return False


class _Child:
    __slots__ = ("proc_id", "pid", "kill_deadline", "template")

    def __init__(self, proc_id: str, pid: int, template: str | None = None) -> None:
        self.proc_id = proc_id
        self.pid = pid
        self.kill_deadline: float | None = None
        self.template = template


class _Template:
    """Fork server side of one template process."""

    def __init__(self, key: str, pid: int, sock: socket.socket, workdir: str) -> None:
        self.key = key
        self.pid = pid
        self.sock = sock
        self.workdir = workdir
        # cgroup: waiting for the node to place the template in the template memory group.
        # importing: waiting for ``ready``. ready: forks plugin processes.
        self.state = "cgroup"
        self.deadline: float | None = time.monotonic() + TEMPLATE_PREPARE_S
        self.waiting: list[dict[str, Any]] = []
        self.inflight: dict[str, dict[str, Any]] = {}
        self.inbuf = bytearray()
        self.idle_since: float | None = None


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
        self.templates_enabled = os.environ.get("FISH_SIDECAR_TEMPLATES", "1") != "0" and enable_subreaper()
        self.templates_dir = os.path.join(os.path.dirname(os.path.abspath(socket_path)), "templates")
        self.templates: dict[str, _Template] = {}
        self.template_pids: dict[int, _Template] = {}
        # proc_id of a spawn handed to a template and not yet forked.
        self.routed: dict[str, _Template] = {}
        # Template key and the reason its import cannot be shared.
        self.unshareable: dict[str, str] = {}
        # Exit status and reap time of adopted processes reaped before their template reported the fork.
        self.early_exits: dict[int, tuple[int, float]] = {}
        self.cancelled: set[str] = set()
        self.template_ids = itertools.count(1)

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
        shutil.rmtree(self.templates_dir, ignore_errors=True)
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
        elif op == "template_ready":
            self.on_template_ready(message)
        else:
            log.warning("unknown control op %r", op)

    def spawn(self, spec: dict[str, Any]) -> None:
        proc_id = str(spec.get("proc_id") or "")
        missing = [k for k in ("proc_id", "plugin_dir", "socket") if not spec.get(k)]
        if missing:
            self.send({"op": "error", "proc_id": proc_id, "message": f"spawn is missing {', '.join(missing)}"})
            return
        if proc_id in self.by_proc or proc_id in self.routed:
            self.send({"op": "error", "proc_id": proc_id, "message": "proc_id is already running"})
            return
        if not os.path.isdir(str(spec["plugin_dir"])):
            self.send({"op": "error", "proc_id": proc_id, "message": f"plugin_dir {spec['plugin_dir']} does not exist"})
            return
        self.cancelled.discard(proc_id)
        key = self.template_key(spec)
        if key is not None:
            reason = self.unshareable.get(key)
            if reason is None:
                self.route(key, spec)
                return
            spec = {**spec, "template_note": reason}
        self.spawn_direct(spec)

    def spawn_direct(self, spec: dict[str, Any]) -> None:
        proc_id = str(spec.get("proc_id") or "")
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
            self.close_inherited()
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

    def close_inherited(self) -> None:
        """Resets signal handling and closes server descriptors in a forked process."""
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
        for tpl in self.templates.values():
            tpl.sock.close()

    def template_key(self, spec: dict[str, Any]) -> str | None:
        share = spec.get("share_key")
        if not self.templates_enabled or not isinstance(share, str) or not share:
            return None
        return f"{share}/{spec.get('plugin_name') or 'plugin'}"

    def route(self, key: str, spec: dict[str, Any]) -> None:
        proc_id = str(spec["proc_id"])
        tpl = self.templates.get(key)
        if tpl is None:
            tpl = self.start_template(key, spec)
            if tpl is None:
                self.spawn_direct(spec)
                return
        self.routed[proc_id] = tpl
        tpl.idle_since = None
        if tpl.state == "ready":
            self.send_to_template(tpl, spec)
        else:
            tpl.waiting.append(spec)

    def start_template(self, key: str, spec: dict[str, Any]) -> _Template | None:
        workdir = os.path.join(self.templates_dir, str(next(self.template_ids)))
        try:
            os.makedirs(workdir)
            parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        except OSError as e:
            log.warning("template setup failed: %s", e)
            return None
        sys.stdout.flush()
        sys.stderr.flush()
        try:
            pid = os.fork()
        except OSError as e:
            parent.close()
            child.close()
            log.warning("template fork failed: %s", e)
            return None
        if pid == 0:
            code = 1
            try:
                parent.close()
                self.close_inherited()
                os.setsid()
                from fish_sidecar import template

                code = template.serve(child, {k: spec.get(k) for k in ("plugin_dir", "venv_site_packages", "plugin_name")}, workdir)
            except BaseException:
                try:
                    traceback.print_exc(file=sys.__stderr__)
                except Exception:
                    pass
            finally:
                os._exit(code)
        child.close()
        parent.settimeout(TEMPLATE_SEND_TIMEOUT_S)
        tpl = _Template(key, pid, parent, workdir)
        self.templates[key] = tpl
        self.template_pids[pid] = tpl
        self.sel.register(parent, selectors.EVENT_READ, lambda _k, t=tpl: self.on_template(t))
        self.send({"op": "template", "key": key, "pid": pid})
        return tpl

    def on_template_ready(self, message: dict[str, Any]) -> None:
        tpl = self.templates.get(str(message.get("key") or ""))
        if tpl is None or tpl.pid != message.get("pid") or tpl.state != "cgroup":
            return
        if not message.get("ok"):
            log.warning("template %s was not placed in its memory group: %s", tpl.key, message.get("message"))
            self.drop_template(tpl)
            return
        tpl.state = "importing"
        if not self.write_template(tpl, b"go\n"):
            self.drop_template(tpl)

    def write_template(self, tpl: _Template, data: bytes) -> bool:
        try:
            tpl.sock.sendall(data)
            return True
        except OSError as e:
            log.warning("template %s write failed: %s", tpl.key, e)
            return False

    def send_to_template(self, tpl: _Template, spec: dict[str, Any]) -> None:
        proc_id = str(spec["proc_id"])
        tpl.inflight[proc_id] = spec
        if not self.write_template(tpl, protocol.encode(spec)):
            self.drop_template(tpl)

    def on_template(self, tpl: _Template) -> None:
        try:
            data = tpl.sock.recv(65536)
        except (BlockingIOError, TimeoutError):
            return
        except OSError:
            data = b""
        if not data:
            self.drop_template(tpl)
            return
        tpl.inbuf += data
        while self.templates.get(tpl.key) is tpl:
            nl = tpl.inbuf.find(b"\n")
            if nl < 0:
                break
            line = bytes(tpl.inbuf[:nl])
            del tpl.inbuf[: nl + 1]
            try:
                message = protocol.decode(line)
            except ValueError:
                continue
            if message is not None:
                self.on_template_message(tpl, message)

    def on_template_message(self, tpl: _Template, message: dict[str, Any]) -> None:
        op = message.get("op")
        if op == "ready" and tpl.state == "importing":
            tpl.state = "ready"
            tpl.deadline = None
            log.info("template %s ready", tpl.key)
            waiting, tpl.waiting = tpl.waiting, []
            for spec in waiting:
                if self.templates.get(tpl.key) is tpl:
                    self.send_to_template(tpl, spec)
                else:
                    self.respawn_direct(spec)
        elif op == "failed":
            reason = str(message.get("reason") or "插件导入无法共享")
            log.info("template %s cannot be shared: %s", tpl.key, reason)
            if len(self.unshareable) < MAX_UNSHAREABLE:
                self.unshareable[tpl.key] = reason
            self.drop_template(tpl, note=reason)
        elif op == "forked":
            proc_id = str(message.get("proc_id") or "")
            pid = int(message.get("pid") or 0)
            if tpl.inflight.pop(proc_id, None) is None or pid <= 0:
                return
            self.routed.pop(proc_id, None)
            child = _Child(proc_id, pid, tpl.key)
            self.children[pid] = child
            self.by_proc[proc_id] = child
            self.send({"op": "spawned", "proc_id": proc_id, "pid": pid})
            if proc_id in self.cancelled:
                self.cancelled.discard(proc_id)
                self.kill(proc_id)
            early = self.early_exits.pop(pid, None)
            if early is not None and time.monotonic() - early[1] < EARLY_EXIT_TTL_S:
                self.finish_child(pid, early[0])
        elif op == "fork_failed":
            spec = tpl.inflight.pop(str(message.get("proc_id") or ""), None)
            if spec is not None:
                log.warning("template %s fork failed: %s", tpl.key, message.get("message"))
                self.respawn_direct(spec)

    def respawn_direct(self, spec: dict[str, Any], note: str | None = None) -> None:
        proc_id = str(spec.get("proc_id") or "")
        self.routed.pop(proc_id, None)
        if proc_id in self.cancelled:
            self.cancelled.discard(proc_id)
            self.send({"op": "error", "proc_id": proc_id, "message": "spawn was cancelled"})
            return
        self.spawn_direct({**spec, "template_note": note} if note else spec)

    def drop_template(self, tpl: _Template, note: str | None = None) -> None:
        """Stops a template. Spawns it had not started yet run directly."""
        if self.templates.get(tpl.key) is tpl:
            del self.templates[tpl.key]
        try:
            self.sel.unregister(tpl.sock)
        except (KeyError, ValueError):
            pass
        tpl.sock.close()
        try:
            os.kill(tpl.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
        shutil.rmtree(tpl.workdir, ignore_errors=True)
        waiting, tpl.waiting = tpl.waiting, []
        for spec in waiting:
            self.respawn_direct(spec, note)
        inflight, tpl.inflight = tpl.inflight, {}
        for proc_id in inflight:
            # The template may have forked this process before it stopped, so it is not started again.
            self.routed.pop(proc_id, None)
            self.cancelled.discard(proc_id)
            self.send({"op": "error", "proc_id": proc_id, "message": "插件模板进程在启动插件时退出"})

    def check_templates(self) -> float | None:
        """Enforces template deadlines and stops idle templates. Returns seconds until the next check."""
        now = time.monotonic()
        nearest: float | None = None
        for pid, (_status, at) in list(self.early_exits.items()):
            if now - at >= EARLY_EXIT_TTL_S:
                del self.early_exits[pid]
        users = {c.template for c in self.children.values() if c.template is not None}
        for tpl in list(self.templates.values()):
            if tpl.deadline is not None:
                if now >= tpl.deadline:
                    reason = "插件导入超时" if tpl.state == "importing" else "节点没有确认模板的内存组"
                    log.warning("template %s: %s", tpl.key, reason)
                    if tpl.state == "importing" and len(self.unshareable) < MAX_UNSHAREABLE:
                        self.unshareable[tpl.key] = reason
                    self.drop_template(tpl, note=reason)
                    continue
                left = tpl.deadline - now
            elif tpl.key in users or tpl.inflight or tpl.waiting:
                tpl.idle_since = None
                continue
            else:
                if tpl.idle_since is None:
                    tpl.idle_since = now
                left = tpl.idle_since + TEMPLATE_IDLE_S - now
                if left <= 0:
                    log.info("template %s idle, stopping", tpl.key)
                    self.drop_template(tpl)
                    continue
            nearest = left if nearest is None else min(nearest, left)
        return nearest

    def kill(self, proc_id: str) -> None:
        tpl = self.routed.get(proc_id)
        if tpl is not None:
            for spec in tpl.waiting:
                if spec.get("proc_id") == proc_id:
                    tpl.waiting.remove(spec)
                    self.routed.pop(proc_id, None)
                    self.send({"op": "error", "proc_id": proc_id, "message": "spawn was cancelled"})
                    return
            self.cancelled.add(proc_id)
            return
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
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return
            except InterruptedError:
                continue
            if pid == 0:
                return
            if pid in self.children:
                self.finish_child(pid, status)
                continue
            tpl = self.template_pids.pop(pid, None)
            if tpl is not None:
                if self.templates.get(tpl.key) is tpl:
                    log.warning("template %s exited in state %s", tpl.key, tpl.state)
                    note = None
                    if tpl.state != "ready":
                        # For example killed for exceeding the template memory group while importing.
                        note = "导入插件的模板进程意外退出"
                        if len(self.unshareable) < MAX_UNSHAREABLE:
                            self.unshareable[tpl.key] = note
                    self.drop_template(tpl, note=note)
                continue
            if self.templates and len(self.early_exits) < MAX_EARLY_EXITS:
                self.early_exits[pid] = (status, time.monotonic())

    def finish_child(self, pid: int, status: int) -> None:
        child = self.children.pop(pid, None)
        if child is None:
            return
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
            if self.templates:
                left = self.check_templates()
                if left is not None:
                    timeout = max(0.0, min(timeout, left))
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
        for tpl in list(self.templates.values()):
            self.drop_template(tpl)
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
