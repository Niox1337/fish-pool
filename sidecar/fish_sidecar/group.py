"""Group process: runs several light plugins of one bot in one process.

The node starts the process with ``{"op": "spawn", "mode": "group", ...}``. The
process connects to the spawn ``socket``, reads the memory limit byte when
``wait_for_memory_limit`` is set and sends ``{"op": "ready"}``. Control ops from
the node:

- ``{"op": "load", "proc_id", "plugin_dir", "plugin_name", "data_dir", "socket", "venv_site_packages"}``
  starts a slot. The slot connects to its own ``socket`` and speaks the plugin
  process protocol of ``fish_sidecar.child``, starting with the config line.
- ``{"op": "unload", "proc_id"}`` stops a slot.
- ``{"op": "exit"}`` stops every slot and ends the process.

The process sends ``{"op": "slot_exited", "proc_id", "code", "drain"}`` when a
slot ends. ``code`` uses the exit codes of ``fish_sidecar.child``. A slot whose
plugin cannot share a process ends with ``EXIT_UNGROUPABLE`` before ``hello``,
or after it when the plugin blocks the event loop. ``drain`` is true when the
slot ran plugin code and ended abnormally, so the process may hold leftovers of
that plugin and should take no new slots.

Each slot runs in its own ``contextvars`` context with ``runtime.slot`` set.
AstrBot's root path, database, preferences, ``StarTools`` context, log target
and ``sys.stdout``/``sys.stderr`` resolve through that variable.
"""

from __future__ import annotations

import asyncio
import contextvars
import importlib.metadata
import importlib.util
import io
import logging
import os
import re
import sys
import threading
import time
import tokenize
import traceback
from pathlib import Path
from typing import Any

from fish_sidecar import astrbot_boot, loader, protocol, runtime
from fish_sidecar.child import EXIT_OK, EXIT_UNGROUPABLE, ChildRuntime, Ungroupable, _idle_thread, _LineWriter

log = logging.getLogger("astrbot")

STALL_S = 5.0
HEARTBEAT_S = 0.5
EXIT_TIMEOUT_S = 10.0
TERMINATE_WAIT_S = 3.0
MAX_SOURCE_FILES = 300
MAX_SOURCE_BYTES = 3 * 1024 * 1024

# Source patterns of plugins that keep background work, block the event loop or
# change process wide state. Comments are removed before matching.
# shortcut: text patterns miss indirect uses and flag harmless ones, replace with an AST pass when false results show up in plugin logs.
BLOCKERS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\bThread\s*\(|start_new_thread|\bTimer\s*\(|ThreadPoolExecutor|ProcessPoolExecutor"), "启动线程"),
    (
        re.compile(r"\bcreate_task\b|\bensure_future\b|\bregister_task\b|\bTaskGroup\b|\bcall_later\b|\bcall_at\b"),
        "启动后台任务",
    ),
    (re.compile(r"apscheduler|\bcron_manager\b|\badd_job\b|^\s*(?:import|from)\s+schedule\b", re.M), "定时任务"),
    (re.compile(r"\btime\.sleep\s*\(|^\s*from\s+time\s+import\s+[^\n]*\bsleep\b", re.M), "阻塞等待"),
    (
        re.compile(r"^\s*(?:import|from)\s+(?:requests|urllib3|http\.client|urllib\.request)\b|\burlopen\s*\(", re.M),
        "同步网络请求",
    ),
    (re.compile(r"\bsubprocess\b|\bos\.system\b|\bos\.popen\b|\bos\.fork\b|\bmultiprocessing\b|\bos\.exec"), "子进程"),
    (
        re.compile(
            r"\bos\.chdir\b|\bos\.putenv\b|\bos\.environ\s*\[[^\]]*\]\s*=(?!=)|\bos\.environ\.(?:update|setdefault|pop)\b"
        ),
        "修改进程环境",
    ),
    (
        re.compile(
            r"\bgetcwd\b|\bPath\.cwd\b|\bPath\s*\(\s*[rbuf]*['\"](?![/{])|\bopen\s*\(\s*[rbuf]*['\"](?![/{])|\bos\.(?:path\.\w+|listdir|makedirs|mkdir|remove|unlink|scandir|rename|replace)\s*\(\s*[rbuf]*['\"](?![/{])|['\"](?:\./|data/)"
        ),
        "使用相对路径",
    ),
    (
        re.compile(
            r"\bsys\.(?:stdout|stderr|stdin)\s*=(?!=)|\bsys\.path\b|\bsys\.exit\b|\bos\._exit\b|\bsignal\.|\batexit\b"
        ),
        "修改进程状态",
    ),
    (re.compile(r"\bsession_waiter\b|\bSessionWaiter\b"), "会话等待"),
    (re.compile(r"\bkb_manager\b|\bkb_mgr\b|\bknowledge_base\b"), "知识库"),
]


def current_slot() -> SlotRuntime | None:
    rt = runtime.slot.get()
    return rt if isinstance(rt, SlotRuntime) else None


def _source(path: Path) -> str:
    """File text without comments."""
    try:
        with path.open("rb") as f:
            tokens = list(tokenize.tokenize(f.readline))
        return tokenize.untokenize(t for t in tokens if t.type != tokenize.COMMENT).decode("utf-8", "replace")
    except (tokenize.TokenError, SyntaxError, UnicodeDecodeError, ValueError):
        return path.read_text("utf-8", "replace")


# Directories and files that hold tests or documentation, which the plugin does not import.
SKIPPED_DIRS = {"tests", "test", "docs", "examples"}


def _scanned(rel: Path) -> bool:
    if any(part.startswith((".", "__pycache__")) or part in SKIPPED_DIRS for part in rel.parts[:-1]):
        return False
    name = rel.name
    return not (name.startswith("test_") or name.endswith("_test.py") or name == "conftest.py")


def static_blockers(plugin_dir: Path) -> list[str]:
    """Reasons from ``BLOCKERS`` found in the plugin's Python files, each with the first file that has it."""
    files = [p for p in plugin_dir.rglob("*.py") if _scanned(p.relative_to(plugin_dir))]
    if len(files) > MAX_SOURCE_FILES or sum(p.stat().st_size for p in files) > MAX_SOURCE_BYTES:
        return ["代码量较大"]
    found: dict[str, str] = {}
    for path in sorted(files):
        text = _source(path)
        for pattern, reason in BLOCKERS:
            if reason not in found and pattern.search(text):
                found[reason] = str(path.relative_to(plugin_dir))
    return [f"{reason} ({where})" for reason, where in found.items()]


def top_level_names(plugin_dir: Path) -> set[str]:
    """Modules the plugin directory provides when it is on ``sys.path``, without ``main``."""
    names: set[str] = set()
    for entry in plugin_dir.iterdir():
        if entry.name.startswith((".", "_")) or entry.name in SKIPPED_DIRS:
            continue
        if entry.is_file() and entry.suffix == ".py":
            names.add(entry.stem)
        elif entry.is_dir():
            names.add(entry.name)
    names.discard("main")
    return {n for n in names if n.isidentifier()}


def module_conflicts(plugin_dir: Path) -> list[str]:
    """Top level modules of the plugin that already resolve to code outside its directory."""
    root = str(plugin_dir.resolve())
    out = []
    for name in sorted(top_level_names(plugin_dir)):
        try:
            spec = importlib.util.find_spec(name)
        except (ImportError, ValueError):
            out.append(name)
            continue
        if spec is None:
            continue
        places = [spec.origin or "", *(spec.submodule_search_locations or [])]
        if not any(p and os.path.realpath(p).startswith(root + os.sep) for p in places):
            out.append(name)
    return out


# Entries of a uv venv that every venv has.
VENV_BUILTINS = {"_virtualenv"}


def venv_modules(site_packages: Path) -> set[str]:
    """Top level modules in ``site_packages``, without the entries every venv has."""
    names: set[str] = set()
    for entry in site_packages.iterdir():
        name = entry.name
        if entry.is_dir():
            if "." in name or name == "__pycache__":
                continue
        elif name.endswith(".py"):
            name = name[:-3]
        elif name.endswith(".so"):
            name = name.split(".", 1)[0]
        else:
            continue
        if name.isidentifier() and name not in VENV_BUILTINS:
            names.add(name)
    return names


def _venv_versions(site_packages: Path) -> dict[str, tuple[str, str]]:
    """Distribution name and version of each top level module the venv installs."""
    out: dict[str, tuple[str, str]] = {}
    for dist in importlib.metadata.distributions(path=[str(site_packages)]):
        name, version = dist.metadata["Name"] or "", dist.version or ""
        tops = set((dist.read_text("top_level.txt") or "").split())
        for file in dist.files or []:
            first = file.parts[0] if file.parts else ""
            if first and not first.endswith((".dist-info", ".data")) and first != "..":
                tops.add(first.split(".", 1)[0] if first.endswith((".py", ".so")) else first)
        for top in tops:
            out[top] = (name, version)
    return out


def venv_conflicts(site_packages: Path) -> list[str]:
    """Problems that keep the venv's packages from sharing a process with other code: startup files,
    and top level modules that already resolve outside the venv to a different version."""
    out = sorted(p.name for p in site_packages.glob("*.pth") if p.stem not in VENV_BUILTINS)
    if out:
        return [f"启动文件 {', '.join(out)}"]
    versions: dict[str, tuple[str, str]] | None = None
    for name in sorted(venv_modules(site_packages)):
        try:
            spec = importlib.util.find_spec(name)
        except (ImportError, ValueError):
            spec = None
        if spec is None:
            continue
        if versions is None:
            versions = _venv_versions(site_packages)
        dist, version = versions.get(name, ("", ""))
        try:
            same = bool(dist) and importlib.metadata.version(dist) == version
        except importlib.metadata.PackageNotFoundError:
            same = False
        if not same:
            out.append(f"{dist or name} {version}".strip())
    return out


def _owned(path: str | None, prefix: str) -> bool:
    return bool(path) and (path == prefix or path.startswith(prefix + "."))


def purge(package: str, plugin_dir: Path, site_packages: str | None = None) -> None:
    """Removes a plugin's modules, venv modules, handlers, tools and metadata from the process registries."""
    from astrbot.core.provider.register import llm_tools
    from astrbot.core.star.star import star_map, star_registry
    from astrbot.core.star.star_handler import star_handlers_registry

    for md in [m for m in star_handlers_registry if _owned(m.handler_module_path, package)]:
        star_handlers_registry.remove(md)
    llm_tools.func_list[:] = [
        t for t in llm_tools.func_list if not _owned(getattr(t, "handler_module_path", None), package)
    ]
    for key in [k for k in star_map if _owned(k, package)]:
        del star_map[key]
    star_registry[:] = [m for m in star_registry if not _owned(m.module_path, package)]
    for key in [k for k in sys.modules if _owned(k, package)]:
        del sys.modules[key]
    entries = [str(plugin_dir)]
    if site_packages:
        venv = os.path.realpath(site_packages) + os.sep
        for key, module in list(sys.modules.items()):
            origin = getattr(module, "__file__", None)
            if origin and os.path.realpath(origin).startswith(venv):
                del sys.modules[key]
        entries.append(site_packages)
    for entry in entries:
        while entry in sys.path:
            sys.path.remove(entry)
    importlib.invalidate_caches()


class _ContextStream(io.TextIOBase):
    """Text stream that writes to the current slot's stream, or to ``fallback`` outside a slot."""

    def __init__(self, attr: str, fallback: Any) -> None:
        self.attr = attr
        self.fallback = fallback

    def _target(self) -> Any:
        slot = current_slot()
        return getattr(slot, self.attr, None) or self.fallback

    def writable(self) -> bool:
        return True

    def write(self, s: str) -> int:
        return self._target().write(s)

    def flush(self) -> None:
        self._target().flush()


def _make_contextual(obj: Any, attr: str) -> None:
    """Changes the class of ``obj`` so attribute access goes to the current slot's ``attr`` object."""
    base = type(obj)

    def target(self: Any) -> Any:
        slot = current_slot()
        found = getattr(slot, attr, None) if slot is not None else None
        return None if found is self else found

    def __getattribute__(self: Any, name: str) -> Any:
        found = target(self)
        return object.__getattribute__(self, name) if found is None else getattr(found, name)

    def __setattr__(self: Any, name: str, value: Any) -> None:
        found = target(self)
        if found is None:
            object.__setattr__(self, name, value)
        else:
            setattr(found, name, value)

    proxy = type(base)(
        base.__name__,
        (base,),
        {"__getattribute__": __getattribute__, "__setattr__": __setattr__, "__module__": base.__module__},
    )
    obj.__class__ = proxy


class _SlotStarContext:
    """``StarTools._context`` descriptor that returns the current slot's plugin Context."""

    def __get__(self, obj: Any, owner: Any = None) -> Any:
        slot = current_slot()
        return slot.star_context if slot is not None else None


_installed = False


def install() -> None:
    """Points process wide AstrBot state at the current slot. Call once in the group process."""
    global _installed
    if _installed:
        return
    _installed = True
    import astrbot.core as core
    from astrbot.core.star.star_tools import StarTools
    from astrbot.core.utils import astrbot_path

    original = astrbot_path.get_astrbot_root
    fallback = type(original)(original.__code__, original.__globals__, "original_get_astrbot_root")

    def slot_root() -> str:
        slot = current_slot()
        return slot.root if slot is not None and slot.root else fallback()

    def get_astrbot_root() -> str:
        return _fish_slot_root()  # type: ignore[name-defined]  # noqa: F821

    # Modules that imported the function by name keep the same function object, so its code is replaced.
    astrbot_path._fish_slot_root = slot_root  # type: ignore[attr-defined]
    original.__code__ = get_astrbot_root.__code__

    _make_contextual(core.db_helper, "db")
    _make_contextual(core.sp, "sp")

    def initialize(cls: Any, context: Any) -> None:
        slot = current_slot()
        if slot is not None:
            slot.star_context = context

    type.__setattr__(StarTools, "_context", _SlotStarContext())
    StarTools.initialize = classmethod(initialize)  # type: ignore[method-assign,assignment]

    def sink(level: str, message: str) -> None:
        slot = current_slot()
        if slot is not None:
            slot.log_sink(level, message)
        elif level != "debug":
            print(f"{level.upper()} {message}", file=sys.__stderr__, flush=True)

    astrbot_boot.route_logs(sink)
    sys.stdout = _ContextStream("stdout", sys.__stdout__)
    sys.stderr = _ContextStream("stderr", sys.__stderr__)


class SlotRuntime(ChildRuntime):
    """One plugin inside a group process."""

    def __init__(self, spec: dict[str, Any], host: GroupHost) -> None:
        super().__init__(spec)
        self.host = host
        self.root: str | None = None
        self.db: Any = None
        self.sp: Any = None
        self.star_context: Any = None
        self.stdout: _LineWriter | None = None
        self.stderr: _LineWriter | None = None
        self.debug = bool(spec.get("debug"))
        self.package: str | None = None
        self.venv: str | None = None
        self.forced_code: int | None = None

    def attach(self) -> None:
        self.stdout = _LineWriter(self, "info")
        self.stderr = _LineWriter(self, "warning")

    def log_sink(self, level: str, message: str) -> None:
        if level != "debug" or self.debug:
            self.write_threadsafe({"op": "log", "level": level, "message": message})

    def module_prefix(self, package: str) -> str | None:
        return package

    def owns_context(self, ctx: Any) -> bool:
        return ctx is not None and ctx.get(runtime.slot) is self

    def own_fds(self) -> set[int]:
        fds = super().own_fds()
        return fds | self.host.own_fds()

    def enter_root(self) -> None:
        """Prepares the data dir the way ``astrbot_boot.enter_plugin_root`` does and opens this slot's database."""
        from astrbot.core.db.sqlite import SQLiteDatabase
        from astrbot.core.utils.shared_preferences import SharedPreferences

        root = self.data_dir
        root.mkdir(parents=True, exist_ok=True)
        link = root / "data"
        if not os.path.lexists(link):
            os.symlink(".", link, target_is_directory=True)
        (root / "config").mkdir(exist_ok=True)
        plugin_link = root / "plugins" / self.plugin_name
        try:
            plugin_link.parent.mkdir(parents=True, exist_ok=True)
            if not plugin_link.exists() and not plugin_link.is_symlink():
                plugin_link.symlink_to(self.plugin_dir, target_is_directory=True)
        except OSError:
            pass
        self.root = os.path.realpath(root)
        self.db = SQLiteDatabase(os.path.join(self.root, "data_v4.db"))
        self.sp = SharedPreferences(db_helper=self.db)

    def import_plugin(self) -> tuple[str, Any]:
        blockers = static_blockers(self.plugin_dir)
        if blockers:
            raise Ungroupable("代码包含 " + "、".join(blockers))
        conflicts = module_conflicts(self.plugin_dir)
        if conflicts:
            raise Ungroupable("模块名与已加载的代码重名: " + ", ".join(conflicts))
        venv = self.spec.get("venv_site_packages")
        if venv and Path(venv).is_dir():
            conflicts = venv_conflicts(Path(venv))
            if conflicts:
                raise Ungroupable("依赖与同进程的其他代码不兼容: " + ", ".join(conflicts))
        else:
            venv = None
        self.enter_root()
        self.package = f"{loader.PACKAGE_ROOT}.{loader.safe_identifier(self.plugin_name)}"
        self.venv = venv
        if venv:
            # The venv modules have names no other code in the process uses, or the same version, so the venv goes last.
            sys.path.append(venv)
        loader.add_import_paths(self.plugin_dir, None)
        return loader.import_plugin_package(self.plugin_dir, self.plugin_name)

    async def load(self) -> None:
        before = set(threading.enumerate())
        async with self.host.load_lock:
            await super().load()
        await asyncio.sleep(0)
        threads = [t.name for t in threading.enumerate() if t not in before and t.is_alive() and not _idle_thread(t)]
        if threads:
            raise Ungroupable("加载时启动了线程 " + ", ".join(sorted(threads)[:3]))
        if self.queued_tasks:
            raise Ungroupable("加载时注册了后台任务")
        own = {self.main_task, self.reader_task, asyncio.current_task()}
        tasks = [
            t
            for t in asyncio.all_tasks(self.loop)
            if t not in own and not t.done() and self.owns_context(t.get_context())
        ]
        if tasks:
            raise Ungroupable(f"加载后仍有 {len(tasks)} 个后台任务")

    def demote(self, reason: str) -> None:
        """Stops the slot with ``EXIT_UNGROUPABLE`` so the node runs the plugin alone."""
        if self.forced_code is not None:
            return
        self.forced_code = EXIT_UNGROUPABLE
        self.write({"op": "log", "level": "warning", "message": f"插件改为单独运行: {reason}"})
        self.stopping.set()

    async def run(self) -> int:
        code = await super().run()
        if code == EXIT_OK and self.forced_code is not None:
            return self.forced_code
        return code

    async def cleanup(self) -> None:
        """Cancels the slot's remaining tasks, closes its database and removes the plugin from the registries."""
        current = asyncio.current_task()
        tasks = [
            t
            for t in asyncio.all_tasks(self.loop)
            if t is not current and not t.done() and self.owns_context(t.get_context())
        ]
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.wait(tasks, timeout=TERMINATE_WAIT_S)
        if self.writer is not None and not self.writer.is_closing():
            self.writer.close()
        if self.sp is not None:
            try:
                await self.sp.close()
            except Exception as e:
                log.debug("preferences close failed: %s", e)
        if self.db is not None:
            try:
                await self.db.engine.dispose()
            except Exception as e:
                log.debug("database close failed: %s", e)
        if self.package is not None:
            purge(self.package, self.plugin_dir, self.venv)


class GroupHost:
    """Control side of the group process."""

    def __init__(self, spec: dict[str, Any]) -> None:
        self.spec = spec
        self.workdir = Path(spec.get("data_dir") or spec["plugin_dir"]).resolve()
        self.loop = asyncio.get_running_loop()
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None
        self.slots: dict[str, SlotRuntime] = {}
        self.slot_tasks: dict[str, asyncio.Task] = {}
        self.load_lock = asyncio.Lock()
        self.stopping = asyncio.Event()
        self.heartbeat = time.monotonic()
        self.stalled: dict[SlotRuntime, float] = {}
        self.stalled_lock = threading.Lock()

    def write(self, message: dict[str, Any]) -> None:
        if self.writer is not None and not self.writer.is_closing():
            self.writer.write(protocol.encode(message))

    def own_fds(self) -> set[int]:
        fds: set[int] = set()
        for writer in [self.writer, *(s.writer for s in self.slots.values())]:
            sock = writer.get_extra_info("socket") if writer is not None else None
            if sock is not None:
                fds.add(sock.fileno())
        return fds

    def enter_workdir(self) -> None:
        """Makes the work dir the current dir, HOME and fallback AstrBot root of the process."""
        self.workdir.mkdir(parents=True, exist_ok=True)
        os.chdir(self.workdir)
        home = self.workdir / "home"
        home.mkdir(exist_ok=True)
        os.environ["HOME"] = str(home)
        for var, sub in (
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_CACHE_HOME", ".cache"),
            ("XDG_DATA_HOME", ".local/share"),
        ):
            os.environ[var] = str(home / sub)
        astrbot_boot.enter_plugin_root(str(self.workdir))

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

    def start_slot(self, message: dict[str, Any]) -> None:
        proc_id = str(message.get("proc_id") or "")
        if not proc_id or proc_id in self.slots or not message.get("socket") or not message.get("plugin_dir"):
            self.write({"op": "slot_exited", "proc_id": proc_id, "code": 1, "drain": False})
            return
        spec = {
            "proc_id": proc_id,
            "plugin_dir": message["plugin_dir"],
            "plugin_name": message.get("plugin_name"),
            "data_dir": message.get("data_dir"),
            "venv_site_packages": message.get("venv_site_packages"),
            "socket": message["socket"],
            "debug": message.get("debug"),
            "config_on_socket": True,
        }
        ctx = contextvars.Context()
        slot = ctx.run(SlotRuntime, spec, self)
        ctx.run(runtime.slot.set, slot)
        self.slots[proc_id] = slot
        self.slot_tasks[proc_id] = self.loop.create_task(self.run_slot(slot), context=ctx)

    async def run_slot(self, slot: SlotRuntime) -> None:
        code = 1
        try:
            code = await slot.run()
        except BaseException as e:
            print(
                f"fish group slot {slot.proc_id}: {e}\n{traceback.format_exc(limit=8)}", file=sys.__stderr__, flush=True
            )
        try:
            await slot.cleanup()
        except BaseException as e:
            print(f"fish group slot {slot.proc_id} cleanup: {e}", file=sys.__stderr__, flush=True)
        self.slots.pop(slot.proc_id, None)
        self.slot_tasks.pop(slot.proc_id, None)
        self.write(
            {
                "op": "slot_exited",
                "proc_id": slot.proc_id,
                "code": code,
                "drain": slot.package is not None and code != EXIT_OK and slot.forced_code is None,
            }
        )
        try:
            if self.writer is not None:
                await self.writer.drain()
        except ConnectionError:
            self.stopping.set()

    def on_message(self, message: dict[str, Any]) -> None:
        op = message.get("op")
        if op == "load":
            self.start_slot(message)
        elif op == "unload":
            slot = self.slots.get(str(message.get("proc_id")))
            if slot is not None:
                slot.stopping.set()
        elif op == "exit":
            self.stopping.set()

    async def read_loop(self) -> None:
        assert self.reader is not None
        while not self.stopping.is_set():
            try:
                line = await self.reader.readline()
            except (ConnectionError, ValueError):
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

    def beat(self) -> None:
        """Runs on the event loop every ``HEARTBEAT_S`` and demotes slots that held the loop too long."""
        self.heartbeat = time.monotonic()
        with self.stalled_lock:
            stalled, self.stalled = self.stalled, {}
        for slot, seconds in stalled.items():
            if slot.proc_id in self.slots:
                slot.demote(f"阻塞事件循环 {seconds:.0f} 秒")
        if not self.stopping.is_set():
            self.loop.call_later(HEARTBEAT_S, self.beat)

    def watch(self) -> None:
        """Watchdog thread. Records the slot whose task runs while the loop misses heartbeats."""
        while not self.stopping.is_set():
            time.sleep(HEARTBEAT_S)
            late = time.monotonic() - self.heartbeat
            if late < STALL_S:
                continue
            try:
                task = asyncio.current_task(self.loop)
            except RuntimeError:
                task = None
            slot = task.get_context().get(runtime.slot) if task is not None else None
            if isinstance(slot, SlotRuntime):
                with self.stalled_lock:
                    self.stalled[slot] = max(self.stalled.get(slot, 0.0), late)

    async def run(self) -> int:
        try:
            await self.connect()
            assert self.reader is not None
            if self.spec.get("wait_for_memory_limit"):
                if await asyncio.wait_for(self.reader.readexactly(1), timeout=60) != b"\x01":
                    raise ConnectionError("memory limit was not confirmed")
        except (ConnectionError, TimeoutError, ValueError, asyncio.IncompleteReadError) as e:
            print(f"fish group {self.spec.get('proc_id')}: {e}", file=sys.__stderr__)
            return 3
        self.enter_workdir()
        install()
        self.write({"op": "ready", "pid": os.getpid()})
        self.loop.call_later(HEARTBEAT_S, self.beat)
        watchdog = threading.Thread(target=self.watch, name="fish-group-watchdog", daemon=True)
        watchdog.fish_internal = True  # type: ignore[attr-defined]
        watchdog.start()
        reader_task = self.loop.create_task(self.read_loop())
        await self.stopping.wait()
        for slot in list(self.slots.values()):
            slot.stopping.set()
        tasks = list(self.slot_tasks.values())
        if tasks:
            await asyncio.wait(tasks, timeout=EXIT_TIMEOUT_S)
        reader_task.cancel()
        if self.writer is not None:
            try:
                await self.writer.drain()
            except ConnectionError:
                pass
            self.writer.close()
        return EXIT_OK


async def _main(spec: dict[str, Any]) -> int:
    return await GroupHost(spec).run()


def serve(spec: dict[str, Any]) -> int:
    """Entry point inside a forked process. Returns the exit code."""
    return asyncio.run(_main(spec))
