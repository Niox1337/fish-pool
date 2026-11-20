"""Plugin template process.

The fork server starts one template per shared install of a plugin. The template waits for ``go``,
which the fork server sends after the node moved it into the template memory group, then imports
the plugin once inside an empty working directory. While importing it records anything that would
make the imported state depend on that directory or differ per process: file system access through
relative paths or inside the working directory, randomness, threads, subprocesses, open files,
signal handlers, and module values that contain the working directory path, a clock reading taken
during the import or the template's process id. Any finding makes the
template report ``failed`` and the fork server spawns that plugin directly instead.

After ``ready`` each ``spawn`` line forks a plugin process that already has the plugin modules
loaded. The fork happens twice so the plugin process is reparented to the fork server, a child
subreaper, which reaps it like a directly forked child. Pages of the imported modules stay shared
copy on write between the template and all of its plugin processes.

Control lines from the fork server: ``go``, then spawn specs as JSON. Replies: ``ready``, ``failed``,
``forked`` and ``fork_failed``.
"""

from __future__ import annotations

import datetime
import gc
import io
import logging
import os
import random
import signal
import socket
import sys
import threading
import time
import traceback
import types
from pathlib import Path
from typing import Any

from fish_sidecar import loader, protocol

MAX_IMPORT_LOG_LINES = 200
MAX_SCANNED_VALUES = 200_000
MAX_REASONS = 5

# Log lines produced while the template imported the plugin. Each plugin process replays them.
import_logs: list[tuple[str, str]] = []
# Package name and main module of the imported plugin, set before plugin processes are forked.
preimported: tuple[str, Any] | None = None

_LEVELS = {logging.DEBUG: "debug", logging.INFO: "info", logging.WARNING: "warning", logging.ERROR: "error", logging.CRITICAL: "error"}


class _ImportStream(io.TextIOBase):
    """Stdout or stderr replacement installed during the import.

    While capturing it stores lines in ``import_logs``. Afterwards it forwards to the current
    ``sys.stdout`` or ``sys.stderr``, so handlers created during the import write to the plugin
    process streams after the fork.
    """

    def __init__(self, name: str, level: str) -> None:
        self.name_ = name
        self.level = level
        self.capturing = True
        self._buf = ""

    def writable(self) -> bool:
        return True

    def _target(self) -> Any:
        current = getattr(sys, self.name_)
        return current if current is not self else getattr(sys, f"__{self.name_}__")

    def write(self, s: str) -> int:
        if not self.capturing:
            return self._target().write(s)
        self._buf += s
        *lines, self._buf = self._buf.split("\n")
        for line in lines:
            _remember(self.level, line)
        return len(s)

    def flush(self) -> None:
        if not self.capturing:
            self._target().flush()
            return
        line, self._buf = self._buf, ""
        _remember(self.level, line)


def _remember(level: str, line: str) -> None:
    if line.strip() and len(import_logs) < MAX_IMPORT_LOG_LINES:
        import_logs.append((level, line))


class _CaptureHandler(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.DEBUG)
        self.setFormatter(logging.Formatter("%(message)s"))

    def emit(self, record: logging.LogRecord) -> None:
        try:
            _remember(_LEVELS.get(record.levelno, "info"), self.format(record))
        except Exception:
            pass


_FS_EVENTS: dict[str, tuple[int, ...]] = {
    "open": (0,),
    "os.listdir": (0,),
    "os.scandir": (0,),
    "os.mkdir": (0,),
    "os.remove": (0,),
    "os.rmdir": (0,),
    "os.rename": (0, 1),
    "os.replace": (0, 1),
    "os.symlink": (0, 1),
    "os.link": (0, 1),
    "os.chdir": (0,),
    "os.chmod": (0,),
    "os.chown": (0,),
    "os.truncate": (0,),
    "os.utime": (0,),
    "shutil.copyfile": (0, 1),
    "shutil.rmtree": (0,),
    "glob.glob": (0,),
}
_SPAWN_EVENTS = {"os.system", "subprocess.Popen", "os.posix_spawn", "os.exec", "os.fork", "os.forkpty", "os.spawn"}
_RANDOM_FUNCS = (
    "random", "randint", "randrange", "choice", "choices", "sample", "shuffle", "uniform",
    "getrandbits", "randbytes", "gauss", "normalvariate", "triangular", "betavariate",
)
_SYSRANDOM_METHODS = ("random", "getrandbits", "randbytes")
# Modules whose random state each plugin process seeds again after the fork.
_RESEEDED = ("numpy.random",)
PR_SET_DUMPABLE = 4
# Seconds added on both sides of the import window when matching clock readings.
CLOCK_SLACK_S = 1.0
# Modules whose own process id reads are ignored, because they do not keep the value.
_PID_READERS_IGNORED = ("logging",)

_watch: ImportWatch | None = None
_audit_installed = False


def _audit(event: str, args: tuple) -> None:
    watch = _watch
    if watch is None:
        return
    if event in _FS_EVENTS:
        for i in _FS_EVENTS[event]:
            if i < len(args) and watch.risky_path(args[i], event):
                watch.note(f"导入时访问了与工作目录相关的路径 ({event} {args[i]!r})")
                return
    elif event in _SPAWN_EVENTS:
        watch.note(f"导入时启动了进程 ({event})")


class _Window:
    """Readings of one clock before and after the import."""

    def __init__(self, read: Any) -> None:
        self.read = read
        self.start = read()
        self.end = self.start

    def close(self) -> None:
        self.end = self.read()

    def holds(self, value: float, scale: float = 1.0) -> bool:
        return (self.start - CLOCK_SLACK_S) * scale <= value <= (self.end + CLOCK_SLACK_S) * scale


class ImportWatch:
    """Records what the plugin import does that would not carry over to a forked plugin process."""

    def __init__(self, workdir: str) -> None:
        self.workdir = os.path.realpath(workdir)
        self.reasons: list[str] = []
        self._saved: list[tuple[Any, str, Any]] = []
        self.pid = os.getpid()
        self.pid_read = False
        self.wall = _Window(time.time)
        self.mono = _Window(time.monotonic)
        self.perf = _Window(time.perf_counter)

    def note(self, reason: str) -> None:
        if reason not in self.reasons and len(self.reasons) < MAX_REASONS:
            self.reasons.append(reason)

    def risky_path(self, path: Any, event: str = "") -> bool:
        if path is None:
            return event in ("os.listdir", "os.scandir")
        if isinstance(path, int):
            return False
        try:
            text = os.fsdecode(os.fspath(path))
        except TypeError:
            return False
        if not os.path.isabs(text):
            return True
        full = os.path.normpath(text)
        return full == self.workdir or full.startswith(self.workdir + os.sep)

    def _patch(self, owner: Any, name: str, make: Any) -> None:
        original = getattr(owner, name, None)
        if original is None:
            return
        self._saved.append((owner, name, original))
        setattr(owner, name, make(original))

    def start(self) -> None:
        global _watch, _audit_installed
        if not _audit_installed:
            sys.addaudithook(_audit)
            _audit_installed = True

        def path_guard(original: Any, label: str) -> Any:
            def wrapper(path: Any, *args: Any, **kw: Any) -> Any:
                watch = _watch
                if watch is not None and kw.get("dir_fd") is None and watch.risky_path(path):
                    watch.note(f"导入时访问了与工作目录相关的路径 ({label} {path!r})")
                return original(path, *args, **kw)

            return wrapper

        def random_guard(original: Any, label: str) -> Any:
            def wrapper(*args: Any, **kw: Any) -> Any:
                watch = _watch
                if watch is not None and not _called_from_reseeded():
                    watch.note(f"导入时生成了随机数 ({label})")
                return original(*args, **kw)

            return wrapper

        def seed_guard(original: Any) -> Any:
            def wrapper(self: Any, a: Any = None, *args: Any, **kw: Any) -> Any:
                watch = _watch
                if watch is not None and a is None and not _called_from_reseeded():
                    watch.note("导入时创建了随机数生成器 (random.Random)")
                return original(self, a, *args, **kw)

            return wrapper

        def pid_guard(original: Any) -> Any:
            def wrapper() -> Any:
                watch = _watch
                if watch is not None and not _called_from(_PID_READERS_IGNORED, 2):
                    watch.pid_read = True
                return original()

            return wrapper

        def signal_guard(original: Any) -> Any:
            def wrapper(*args: Any, **kw: Any) -> Any:
                watch = _watch
                if watch is not None:
                    watch.note("导入时设置了信号处理")
                return original(*args, **kw)

            return wrapper

        for name in ("stat", "lstat", "access"):
            self._patch(os, name, lambda o, n=name: path_guard(o, f"os.{n}"))
        self._patch(os, "urandom", lambda o: random_guard(o, "os.urandom"))
        for name in _RANDOM_FUNCS:
            self._patch(random, name, lambda o, n=name: random_guard(o, f"random.{n}"))
        for name in _SYSRANDOM_METHODS:
            self._patch(random.SystemRandom, name, lambda o, n=name: random_guard(o, f"SystemRandom.{n}"))
        self._patch(random.Random, "seed", seed_guard)
        self._patch(signal, "signal", signal_guard)
        self._patch(os, "getpid", pid_guard)
        _watch = self

    def stop(self) -> None:
        global _watch
        _watch = None
        for window in (self.wall, self.mono, self.perf):
            window.close()
        for owner, name, original in reversed(self._saved):
            setattr(owner, name, original)
        self._saved.clear()

    def clock_reading(self, value: Any) -> bool:
        """True when ``value`` looks like a clock read during the import."""
        if isinstance(value, bool):
            return False
        if isinstance(value, float):
            return self.wall.holds(value) or self.mono.holds(value) or self.perf.holds(value)
        if isinstance(value, int):
            return (
                self.wall.holds(value)
                or self.wall.holds(value, 1e3)
                or self.wall.holds(value, 1e9)
                or self.mono.holds(value, 1e9)
                or self.perf.holds(value, 1e9)
            )
        if isinstance(value, datetime.datetime):
            try:
                if value.tzinfo is not None:
                    return self.wall.holds(value.timestamp())
                as_utc = value.replace(tzinfo=datetime.timezone.utc).timestamp()
                return self.wall.holds(value.timestamp()) or self.wall.holds(as_utc)
            except (OverflowError, OSError, ValueError):
                return False
        return False

    def scan_values(self, modules: list[Any]) -> None:
        """Looks in the namespaces of ``modules`` and of their classes, and in the attributes of module level
        objects whose class comes from ``modules``, for values that contain the working directory path, a clock reading taken during the import, or the
        template's process id when the import read it."""
        needle = self.workdir
        budget = MAX_SCANNED_VALUES
        new_names = {getattr(m, "__name__", None) for m in modules}

        def hit(value: Any) -> str | None:
            if isinstance(value, str):
                return "工作目录路径" if needle in value else None
            if isinstance(value, bytes):
                return "工作目录路径" if os.fsencode(needle) in value else None
            if isinstance(value, os.PathLike):
                try:
                    return "工作目录路径" if needle in os.fsdecode(os.fspath(value)) else None
                except TypeError:
                    return None
            if self.clock_reading(value):
                return "导入时的时间"
            if self.pid_read and type(value) is int and value == self.pid:
                return "模板进程号"
            return None

        def check(owner: str, name: str, value: Any) -> bool:
            nonlocal budget
            items: Any
            if isinstance(value, dict):
                items = list(value.keys()) + list(value.values())
            elif isinstance(value, (list, tuple, set, frozenset)):
                items = list(value)
            else:
                items = [value]
            for item in items:
                budget -= 1
                if budget <= 0:
                    return True
                what = hit(item)
                if what is not None:
                    self.note(f"导入时保存了{what} ({owner}.{name})")
                    return True
            return False

        def attributes(value: Any) -> dict[str, Any] | None:
            if isinstance(value, (type, types.ModuleType, types.FunctionType, types.BuiltinFunctionType, types.MethodType)):
                return None
            try:
                attrs = object.__getattribute__(value, "__dict__")
            except Exception:
                return None
            return attrs if isinstance(attrs, dict) else None

        for module in modules:
            namespace = getattr(module, "__dict__", None)
            if not isinstance(namespace, dict):
                continue
            mod_name = getattr(module, "__name__", "?")
            for name, value in list(namespace.items()):
                if name.startswith("__") and name.endswith("__"):
                    continue
                if check(mod_name, name, value):
                    break
                if isinstance(value, type) and getattr(value, "__module__", None) == mod_name:
                    for attr, attr_value in list(vars(value).items()):
                        if check(f"{mod_name}.{name}", attr, attr_value):
                            break
                    continue
                attrs = attributes(value)
                if attrs is not None and getattr(type(value), "__module__", None) in new_names:
                    for attr, attr_value in list(attrs.items()):
                        if check(f"{mod_name}.{name}", attr, attr_value):
                            break
            if budget <= 0:
                self.note("导入后的模块内容太多，无法检查")
                return


def _called_from(modules: tuple[str, ...], depth: int) -> bool:
    """True when the frame ``depth`` levels up, or one above it, belongs to one of ``modules`` or their submodules."""
    try:
        frame = sys._getframe(depth)
    except ValueError:
        return False
    while frame is not None:
        name = frame.f_globals.get("__name__", "")
        if any(name == m or name.startswith(m + ".") for m in modules):
            return True
        frame = frame.f_back
    return False


def _called_from_reseeded() -> bool:
    return _called_from(_RESEEDED, 3)


def reseed() -> None:
    """Seeds the random state of ``_RESEEDED`` modules from the operating system."""
    np_random = sys.modules.get("numpy.random")
    if np_random is not None:
        try:
            np_random.seed()
        except Exception:
            pass


def set_dumpable(flag: bool) -> None:
    """Sets PR_SET_DUMPABLE. A process that is not dumpable cannot be traced by other processes of the same uid."""
    if not sys.platform.startswith("linux"):
        return
    try:
        import ctypes

        ctypes.CDLL(None, use_errno=True).prctl(PR_SET_DUMPABLE, int(flag), 0, 0, 0)
    except Exception:
        pass


def _open_fds() -> set[str]:
    try:
        return set(os.listdir("/proc/self/fd"))
    except OSError:
        return set()


def _task_children() -> list[str]:
    found: list[str] = []
    try:
        for tid in os.listdir("/proc/self/task"):
            try:
                found += Path(f"/proc/self/task/{tid}/children").read_text().split()
            except OSError:
                pass
    except OSError:
        pass
    return found


def _thread_count() -> int:
    try:
        return len(os.listdir("/proc/self/task"))
    except OSError:
        return threading.active_count()


def _workdir_extras(workdir: Path, plugin_name: str) -> list[str]:
    expected = {Path("home"), Path("plugins"), Path("plugins") / plugin_name}
    extras = []
    for root, dirs, files in os.walk(workdir):
        for entry in dirs + files:
            rel = (Path(root) / entry).relative_to(workdir)
            if rel not in expected:
                extras.append(str(rel))
    return extras


def prepare(spec: dict[str, Any], workdir: str) -> dict[str, Any]:
    """Imports the plugin and returns the ``ready`` or ``failed`` reply."""
    global preimported
    plugin_dir = Path(str(spec["plugin_dir"])).resolve()
    plugin_name = str(spec.get("plugin_name") or "plugin")
    work = Path(workdir)
    work.mkdir(parents=True, exist_ok=True)
    loader.enter_data_dir(plugin_dir, work, plugin_name)
    loader.add_import_paths(plugin_dir, spec.get("venv_site_packages"))

    root = logging.getLogger()
    saved_handlers = list(root.handlers)
    saved_level = root.level
    saved_streams = (sys.stdout, sys.stderr)
    for handler in saved_handlers:
        root.removeHandler(handler)
    root.addHandler(_CaptureHandler())
    root.setLevel(logging.INFO)
    out, err = _ImportStream("stdout", "info"), _ImportStream("stderr", "warning")
    sys.stdout, sys.stderr = out, err

    watch = ImportWatch(workdir)
    modules_before = set(sys.modules)
    fds_before = _open_fds()
    threads_before = _thread_count()
    failure: str | None = None
    result: dict[str, Any] = {}

    def run_import() -> None:
        try:
            result["value"] = loader.import_plugin_package(plugin_dir, plugin_name)
        except BaseException as e:
            result["error"] = e

    # The import runs in a worker thread like a plugin process loading directly, which imports
    # through asyncio.to_thread while its event loop runs in the main thread.
    watch.start()
    try:
        worker = threading.Thread(target=run_import, name="fish-template-import")
        worker.start()
        worker.join()
        if "error" in result:
            e = result["error"]
            failure = f"导入插件失败: {type(e).__name__}: {e}"
    finally:
        watch.stop()
        out.flush()
        err.flush()
        out.capturing = err.capturing = False
        sys.stdout, sys.stderr = saved_streams
        for handler in list(root.handlers):
            root.removeHandler(handler)
        for handler in saved_handlers:
            root.addHandler(handler)
        root.setLevel(saved_level)
    if failure is not None:
        return {"op": "failed", "reason": failure}
    package, main = result["value"]

    for _ in range(50):
        if _thread_count() <= threads_before:
            break
        time.sleep(0.02)
    if _thread_count() > threads_before:
        watch.note("导入时启动了线程")
    if _task_children():
        watch.note("导入时启动了子进程")
    new_fds = _open_fds() - fds_before
    if new_fds:
        watch.note(f"导入后留下了 {len(new_fds)} 个打开的文件或连接")
    extras = _workdir_extras(work, plugin_name)
    if extras:
        watch.note(f"导入时在工作目录写入了文件 ({', '.join(sorted(extras)[:3])})")
    watch.scan_values([sys.modules[name] for name in set(sys.modules) - modules_before if name in sys.modules])
    if watch.reasons:
        return {"op": "failed", "reason": "；".join(watch.reasons)}
    preimported = (package, main)
    gc.collect()
    gc.freeze()
    return {"op": "ready"}


def _run_plugin_process(ctl: socket.socket, reader: Any, spec: dict[str, Any]) -> None:
    code = 1
    try:
        reader.close()
        ctl.close()
        for sig in (signal.SIGCHLD, signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(sig, signal.SIG_DFL)
        os.setsid()
        set_dumpable(True)
        reseed()
        from fish_sidecar import child

        spec["preimported"] = True
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


def _fork(ctl: socket.socket, reader: Any, spec: dict[str, Any]) -> dict[str, Any]:
    proc_id = str(spec.get("proc_id") or "")
    r, w = os.pipe()
    sys.stdout.flush()
    sys.stderr.flush()
    try:
        middle = os.fork()
    except OSError as e:
        os.close(r)
        os.close(w)
        return {"op": "fork_failed", "proc_id": proc_id, "message": f"fork failed: {e}"}
    if middle == 0:
        os.close(r)
        try:
            pid = os.fork()
        except OSError:
            os._exit(1)
        if pid == 0:
            os.close(w)
            _run_plugin_process(ctl, reader, spec)
        os.write(w, str(pid).encode())
        os._exit(0)
    os.close(w)
    data = b""
    while True:
        chunk = os.read(r, 64)
        if not chunk:
            break
        data += chunk
    os.close(r)
    os.waitpid(middle, 0)
    if not data:
        return {"op": "fork_failed", "proc_id": proc_id, "message": "fork failed"}
    return {"op": "forked", "proc_id": proc_id, "pid": int(data)}


def serve(ctl: socket.socket, spec: dict[str, Any], workdir: str) -> int:
    """Runs the template until the fork server closes the control socket."""
    ctl.settimeout(None)
    set_dumpable(False)
    reader = ctl.makefile("rb")
    if reader.readline().strip() != b"go":
        return 0
    try:
        reply = prepare(spec, workdir)
    except BaseException as e:
        reply = {"op": "failed", "reason": f"模板准备失败: {type(e).__name__}: {e}"}
    ctl.sendall(protocol.encode(reply))
    if reply["op"] != "ready":
        return 0
    while True:
        line = reader.readline()
        if not line:
            return 0
        try:
            message = protocol.decode(line)
        except ValueError:
            continue
        if message is None:
            continue
        ctl.sendall(protocol.encode(_fork(ctl, reader, message)))
