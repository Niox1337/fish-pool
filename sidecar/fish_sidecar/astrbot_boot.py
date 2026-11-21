"""Imports the AstrBot package so the importing process can fork, and points AstrBot at one plugin's data dir after a fork.

Importing ``astrbot.core`` normally starts a console writer thread and an
APScheduler thread, creates an OpenTelemetry tracer provider whose fork handler
runs a thread pool in every forked child, opens a SQLite database and writes a
config file under ``ASTRBOT_ROOT``. ``prepare_import`` replaces the thread
owners with inline versions before AstrBot imports them and sends the import
time files to a scratch root. ``enter_plugin_root`` runs in each plugin process and rebinds the
shared database, preferences and config objects to that process's data dir.
"""

from __future__ import annotations

import importlib.abc
import importlib.machinery
import logging
import mimetypes
import os
import re
import sys
import tempfile
import types
from collections.abc import Callable
from typing import Any

ROOT_ENV = "ASTRBOT_ROOT"


class InlineConsole:
    """Console sink that writes on the calling thread. Same interface as AstrBot's bounded console sink."""

    def __init__(self, stream: Any, capacity: int = 512) -> None:
        self._stream = stream
        self.encoding = getattr(stream, "encoding", None)
        self.dropped = 0

    def isatty(self) -> bool:
        return False

    def write(self, message: str) -> None:
        stream = sys.stderr if self._stream is None else self._stream
        try:
            stream.write(message)
            stream.flush()
        except Exception:
            self.dropped += 1

    def flush(self) -> None:
        return None

    def drain(self, timeout: float = 1.0) -> bool:
        return True

    def close(self) -> None:
        return None

    def get_status(self) -> dict:
        return {
            "capacity": 0,
            "queued_records": 0,
            "unfinished_records": 0,
            "dropped_records": self.dropped,
            "closed": False,
            "writer_alive": False,
        }


class InertScheduler:
    """Background scheduler stand in that records jobs and never starts a thread."""

    running = False

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        self.jobs: list[tuple] = []

    def add_job(self, *args: Any, **kwargs: Any) -> None:
        self.jobs.append((args, kwargs))

    def start(self, *args: Any, **kwargs: Any) -> None:
        return None

    def shutdown(self, *args: Any, **kwargs: Any) -> None:
        return None


def _patch_console(module: types.ModuleType) -> None:
    module.BoundedConsoleSink = InlineConsole  # type: ignore[attr-defined]


def _patch_preferences(module: types.ModuleType) -> None:
    module.BackgroundScheduler = InertScheduler  # type: ignore[attr-defined]


def _patch_tracer_provider(module: types.ModuleType) -> None:
    """Keeps the fork handler of OpenTelemetry tracer providers from starting resource detector threads in the child."""
    import threading

    def _handle_fork(self: Any) -> None:
        self._tracers_lock = threading.Lock()

    module.TracerProvider._handle_fork = _handle_fork  # type: ignore[attr-defined]


def _patch_kb_parser(module: types.ModuleType) -> None:
    """Keeps the pinned MarkItDown plain text result without loading its other converters."""
    from astrbot.core.knowledge_base.parsers.base import BaseParser, ParseResult

    original = module.select_parser

    class PlainTextParser(BaseParser):
        async def parse(self, file_content: bytes, file_name: str) -> ParseResult:
            ext = os.path.splitext(file_name)[1].lower()
            if ext != ".txt" or mimetypes.guess_type(f"file{ext}", strict=False)[0] != "text/plain":
                return await (await original(".txt")).parse(file_content, file_name)

            from charset_normalizer import from_bytes

            text = str(from_bytes(file_content).best())
            text = "\n".join(line.rstrip() for line in re.split(r"\r?\n", text))
            return ParseResult(text=re.sub(r"\n{3,}", "\n\n", text), media=[])

    async def select_parser(ext: str) -> BaseParser:
        if ext == ".txt":
            return PlainTextParser()
        return await original(ext)

    module.select_parser = select_parser


PATCHES: dict[str, Callable[[types.ModuleType], None]] = {
    "astrbot.core.knowledge_base.parsers.util": _patch_kb_parser,
    "astrbot.core.utils.console_sink": _patch_console,
    "astrbot.core.utils.shared_preferences": _patch_preferences,
    "opentelemetry.sdk.trace": _patch_tracer_provider,
}


class _PatchingLoader(importlib.abc.Loader):
    def __init__(self, inner: importlib.abc.Loader, patch: Callable[[types.ModuleType], None]) -> None:
        self.inner = inner
        self.patch = patch

    def create_module(self, spec: importlib.machinery.ModuleSpec) -> types.ModuleType | None:
        return self.inner.create_module(spec)

    def exec_module(self, module: types.ModuleType) -> None:
        self.inner.exec_module(module)
        self.patch(module)

    def __getattr__(self, name: str) -> Any:
        return getattr(self.inner, name)


class _PatchingFinder(importlib.abc.MetaPathFinder):
    """Applies ``PATCHES`` right after the named modules execute, before anything imports names from them."""

    def find_spec(self, fullname: str, path: Any = None, target: Any = None) -> importlib.machinery.ModuleSpec | None:
        patch = PATCHES.get(fullname)
        if patch is None:
            return None
        spec = importlib.machinery.PathFinder.find_spec(fullname, path)
        if spec is None or spec.loader is None:
            return None
        spec.loader = _PatchingLoader(spec.loader, patch)
        return spec


_finder = _PatchingFinder()


def prepare_import() -> None:
    """Call before the first ``import astrbot``. Safe to call more than once."""
    os.environ.setdefault("ASTRBOT_DISABLE_METRICS", "1")
    if not os.environ.get(ROOT_ENV):
        os.environ[ROOT_ENV] = tempfile.mkdtemp(prefix="astrbot-root-")
    if _finder not in sys.meta_path:
        sys.meta_path.insert(0, _finder)


def _rebind(obj: Any, fresh: Any) -> None:
    """Gives ``obj`` the state of ``fresh``, so references that other modules hold see the new state."""
    obj.__dict__.clear()
    obj.__dict__.update(fresh.__dict__)


def enter_plugin_root(root: str) -> None:
    """Points AstrBot at ``root`` in the current process.

    ``root/data`` is a symlink to ``root`` itself, so AstrBot's data dir is ``root``
    and ``root/plugin_data``, ``root/config``, ``root/data_v4.db`` and the relative
    path ``data/...`` from ``root`` all name the same files.
    """
    link = os.path.join(root, "data")
    if not os.path.lexists(link):
        os.symlink(".", link, target_is_directory=True)
    os.environ[ROOT_ENV] = root
    import astrbot.core as core
    from astrbot.core.config.astrbot_config import AstrBotConfig
    from astrbot.core.db.sqlite import SQLiteDatabase
    from astrbot.core.utils.astrbot_path import get_astrbot_data_path
    from astrbot.core.utils.shared_preferences import SharedPreferences

    data = get_astrbot_data_path()
    os.makedirs(os.path.join(data, "config"), exist_ok=True)
    # The database creates its tables on the first session it opens, so a plugin that never uses it does not pay for them.
    _rebind(core.db_helper, SQLiteDatabase(os.path.join(data, "data_v4.db")))
    _rebind(core.sp, SharedPreferences(db_helper=core.db_helper))
    config_path = os.path.join(data, "cmd_config.json")
    if os.path.isfile(config_path):
        config = AstrBotConfig(config_path)
        core.astrbot_config.clear()
        core.astrbot_config.update(config)
        _rebind(core.astrbot_config, config)
    else:
        object.__setattr__(core.astrbot_config, "config_path", config_path)


def warm_database() -> None:
    """Prepares the ORM and asynchronous SQLite paths used during plugin start-up."""
    import asyncio

    from astrbot.core import astrbot_config
    from astrbot.core.astrbot_config_mgr import AstrBotConfigManager
    from astrbot.core.db.sqlite import SQLiteDatabase
    from astrbot.core.knowledge_base.kb_db_sqlite import KBSQLiteDatabase
    from astrbot.core.persona_mgr import PersonaManager
    from astrbot.core.umop_config_router import UmopConfigRouter
    from astrbot.core.utils.shared_preferences import SharedPreferences

    async def warm(root: str) -> None:
        db = SQLiteDatabase(os.path.join(root, "data_v4.db"))
        preferences = SharedPreferences(db_helper=db)
        router = UmopConfigRouter(preferences)
        config = AstrBotConfigManager(astrbot_config, router, preferences)
        try:
            await router.initialize()
            await config.initialize()
            await PersonaManager(db, config).initialize()
            await db.list_cron_jobs()
        finally:
            try:
                await preferences.close()
            finally:
                await db.engine.dispose()

        kb = KBSQLiteDatabase(os.path.join(root, "kb.db"))
        try:
            await kb.initialize()
            await kb.migrate_to_v1()
            await kb.list_kbs()
        finally:
            await kb.close()

    # File-backed SQLite uses the same connection pool and WAL path as plugins.
    with tempfile.TemporaryDirectory(prefix="fish-db-warmup-") as root:
        asyncio.run(warm(root))


LEGACY_KV = "fish_kv.json"


async def migrate_legacy_kv(root: str) -> int:
    """Moves values from ``root/fish_kv.json`` into AstrBot's preference store and renames the file.

    The file maps ``scope\x1fscope_id\x1fkey`` to a value. Values in the
    ``global`` scope are also written to the ``unknown`` scope that AstrBot's
    synchronous ``sp.get`` and ``sp.put`` use when no scope is given. Returns
    the number of values moved.
    """
    import json

    import astrbot.core as core

    path = os.path.join(root, LEGACY_KV)
    if not os.path.isfile(path):
        return 0
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    moved = 0
    for full, value in data.items() if isinstance(data, dict) else ():
        parts = str(full).split("\x1f", 2)
        if len(parts) != 3:
            continue
        scope, scope_id, key = parts
        await core.sp.put_async(scope, scope_id, key, value)
        if (scope, scope_id) == ("global", "global"):
            await core.sp.put_async("unknown", "unknown", key, value)
        moved += 1
    await core.sp.flush()
    os.replace(path, path + ".migrated")
    return moved


def _stderr_target(level: str, text: str) -> None:
    try:
        sys.__stderr__.write(f"{level.upper()} {text}\n")
        sys.__stderr__.flush()
    except Exception:
        pass


_log_target: Callable[[str, str], None] = _stderr_target


def _loguru_sink(message: Any) -> None:
    record = message.record
    name = record["level"].name.lower()
    level = {"trace": "debug", "success": "info", "critical": "error"}.get(name, name)
    if level not in ("debug", "info", "warning", "error"):
        level = "info"
    text = record["message"]
    exc = record["exception"]
    if exc is not None and exc.type is not None:
        import traceback

        text += "\n" + "".join(traceback.format_exception(exc.type, exc.value, exc.traceback)).rstrip()
    _log_target(level, text)


def install_log_sink() -> None:
    """Makes loguru, which carries AstrBot and plugin logs, write only to the current log target.

    Also removes AstrBot's standard library bridge from the root logger, so the
    importing process keeps its own logging setup.
    """
    from loguru import logger as loguru_logger

    from astrbot.core.log import LogManager

    LogManager.GetLogger("astrbot")
    loguru_logger.remove()
    loguru_logger.add(
        _loguru_sink, level="DEBUG", format="{message}", filter=lambda r: not r["extra"].get("is_trace", False)
    )
    root = logging.getLogger()
    for handler in list(root.handlers):
        if getattr(handler, LogManager._LOGGER_HANDLER_FLAG, False):
            root.removeHandler(handler)


def set_log_target(target: Callable[[str, str], None] | None) -> Callable[[str, str], None]:
    """Sets where loguru records go, ``(level, text)``. None restores stderr. Returns the previous target."""
    global _log_target
    previous = _log_target
    _log_target = target or _stderr_target
    return previous


def route_logs(sink: Callable[[str, str], None]) -> None:
    """Sends every log record to ``sink(level, text)``. Standard library records reach loguru through AstrBot's root logger bridge."""
    from astrbot.core.log import LogManager

    set_log_target(sink)
    root = logging.getLogger()
    for handler in list(root.handlers):
        root.removeHandler(handler)
    LogManager._setup_root_bridge()
