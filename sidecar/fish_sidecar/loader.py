"""Loads one AstrBot plugin directory into the current process."""

from __future__ import annotations

import functools
import importlib
import importlib.machinery
import importlib.util
import inspect
import json
import logging
import os
import re
import site
import sys
import types
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from astrbot.core.agent.handoff import FunctionTool, HandoffTool
from astrbot.core.config.astrbot_config import AstrBotConfig
from astrbot.core.provider.register import llm_tools
from astrbot.core.star.base import Star
from astrbot.core.star.context import Context
from astrbot.core.star.star import StarMetadata, star_map, star_registry
from astrbot.core.star.star_handler import star_handlers_registry
from astrbot.core.star.star_tools import StarTools

from fish_sidecar import adapter, astrbot_boot

log = logging.getLogger("astrbot")

PACKAGE_ROOT = "data.plugins"


@dataclass
class LoadedPlugin:
    name: str
    version: str
    package: str
    metadata: StarMetadata
    instance: Star
    context: Context
    config: AstrBotConfig | None


def safe_identifier(name: str) -> str:
    ident = re.sub(r"\W", "_", name) or "plugin"
    return f"p_{ident}" if ident[0].isdigit() else ident


def add_import_paths(plugin_dir: Path, venv_site_packages: str | None) -> None:
    """Puts the plugin venv and plugin dir first on sys.path."""
    if venv_site_packages:
        before = list(sys.path)
        site.addsitedir(venv_site_packages)
        added = [p for p in sys.path if p not in before]
        sys.path[:] = added + before
    sys.path.insert(0, str(plugin_dir))
    importlib.invalidate_caches()


def enter_data_dir(plugin_dir: Path, data_dir: Path, plugin_name: str) -> None:
    """Enters the data dir, points HOME and the XDG dirs inside it and links ``plugins/<name>`` to the source."""
    data_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(data_dir)
    home = data_dir / "home"
    home.mkdir(exist_ok=True)
    os.environ["HOME"] = str(home)
    for var, sub in (("XDG_CONFIG_HOME", ".config"), ("XDG_CACHE_HOME", ".cache"), ("XDG_DATA_HOME", ".local/share")):
        os.environ[var] = str(home / sub)
    astrbot_boot.enter_plugin_root(str(data_dir))
    link = data_dir / "plugins" / plugin_name
    try:
        link.parent.mkdir(parents=True, exist_ok=True)
        if not link.exists() and not link.is_symlink():
            link.symlink_to(plugin_dir, target_is_directory=True)
    except OSError:
        pass


def prepare_paths(plugin_dir: Path, venv_site_packages: str | None, data_dir: Path, plugin_name: str) -> None:
    """Puts the plugin venv and plugin dir first on sys.path, enters the data dir and points HOME at it."""
    add_import_paths(plugin_dir, venv_site_packages)
    enter_data_dir(plugin_dir, data_dir, plugin_name)


def _ensure_namespace(name: str) -> None:
    if name in sys.modules:
        return
    module = types.ModuleType(name)
    module.__path__ = []
    module.__spec__ = importlib.machinery.ModuleSpec(name, None, is_package=True)
    sys.modules[name] = module


def import_plugin_package(plugin_dir: Path, plugin_name: str) -> tuple[str, types.ModuleType]:
    """Imports ``main.py`` as ``data.plugins.<name>.main`` so relative imports work."""
    _ensure_namespace("data")
    _ensure_namespace(PACKAGE_ROOT)
    package = f"{PACKAGE_ROOT}.{safe_identifier(plugin_name)}"
    init = plugin_dir / "__init__.py"
    if init.exists():
        spec = importlib.util.spec_from_file_location(package, init, submodule_search_locations=[str(plugin_dir)])
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[package] = module
        spec.loader.exec_module(module)
    else:
        module = types.ModuleType(package)
        module.__path__ = [str(plugin_dir)]
        module.__package__ = package
        spec = importlib.machinery.ModuleSpec(package, None, is_package=True)
        spec.submodule_search_locations = [str(plugin_dir)]
        module.__spec__ = spec
        sys.modules[package] = module
    if not (plugin_dir / "main.py").exists():
        raise FileNotFoundError(f"main.py not found in {plugin_dir}")
    return package, importlib.import_module(f"{package}.main")


def find_star(package: str, main: types.ModuleType) -> StarMetadata:
    """The plugin class metadata, registered by ``Star.__init_subclass__`` or found by the legacy class name rule."""
    md = star_map.get(main.__name__)
    if md is not None and md.star_cls_type is not None:
        return md
    candidates = [
        m
        for m in star_registry
        if m.star_cls_type is not None and m.module_path and (m.module_path == package or m.module_path.startswith(package + "."))
    ]
    if candidates:
        return candidates[-1]
    legacy = [
        c
        for name, c in inspect.getmembers(main, inspect.isclass)
        if c.__module__ == main.__name__ and (name.lower().endswith("plugin") or name.lower() == "main")
    ]
    if not legacy:
        raise LookupError("no Star subclass found in the plugin")
    md = StarMetadata(star_cls_type=legacy[0], module_path=main.__name__)
    star_map[main.__name__] = md
    star_registry.append(md)
    return md


def read_metadata(plugin_dir: Path) -> dict[str, Any]:
    path = plugin_dir / "metadata.yaml"
    if not path.exists():
        return {}
    try:
        import yaml

        data = yaml.safe_load(path.read_text("utf-8"))
        return data if isinstance(data, dict) else {}
    except Exception as e:
        log.warning("metadata.yaml could not be read: %s", e)
        return {}


def read_schema(plugin_dir: Path) -> dict | None:
    path = plugin_dir / "_conf_schema.json"
    if not path.exists():
        return None
    return json.loads(path.read_text("utf-8-sig"))


def plugin_config(plugin_dir: Path, plugin_name: str, values: dict | None, required: bool = False) -> AstrBotConfig | None:
    """The plugin config: node values in ``config/<name>_config.json``, completed with schema defaults.

    None when the plugin has no ``_conf_schema.json``, unless ``required``.
    """
    schema = read_schema(plugin_dir)
    if schema is None and not required:
        return None
    path = Path.cwd() / "config" / f"{plugin_name}_config.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values or {}, ensure_ascii=False, indent=2), "utf-8")
    return _open_config(path, schema, values)


def _open_config(path: Path, schema: dict | None, values: dict | None) -> AstrBotConfig:
    if schema is None:
        return AstrBotConfig(config_path=str(path), default_config=dict(values or {}))
    return AstrBotConfig(config_path=str(path), schema=schema)


def replace_config(config: AstrBotConfig, values: dict | None) -> None:
    """Writes new node values and reloads them into the same config object."""
    path = Path(config.config_path)
    path.write_text(json.dumps(values or {}, ensure_ascii=False, indent=2), "utf-8")
    fresh = _open_config(path, config.schema, values)
    dict.clear(config)
    dict.update(config, fresh)


def _requires_config(cls: type) -> bool:
    """True when ``__init__`` takes a second parameter without a default."""
    try:
        params = list(inspect.signature(cls.__init__).parameters.values())[1:]
    except (TypeError, ValueError):
        return False
    positional = [p for p in params if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)]
    if any(p.name == "config" and p.default is p.empty for p in params):
        return True
    return len(positional) >= 2 and positional[1].default is positional[1].empty


def _apply_metadata(md: StarMetadata, plugin_dir: Path, plugin_name: str) -> None:
    meta = read_metadata(plugin_dir)
    for field in ("name", "author", "desc", "short_desc", "repo", "display_name", "support_platforms", "astrbot_version"):
        if meta.get(field) is not None:
            setattr(md, field, meta[field])
    if meta.get("description") is not None and not md.desc:
        md.desc = meta["description"]
    if meta.get("version") is not None:
        md.version = str(meta["version"])
    md.name = md.name or plugin_name
    md.author = md.author or "unknown"


def _plugin_tools(module_path: str) -> list[FunctionTool]:
    out: list[FunctionTool] = []
    for tool in llm_tools.func_list:
        if isinstance(tool, HandoffTool):
            agent_tools = getattr(getattr(tool, "agent", None), "tools", None) or []
            out.extend(t for t in agent_tools if isinstance(t, FunctionTool))
        else:
            out.append(tool)
    return out


def bind(md: StarMetadata, instance: Star) -> None:
    """Binds the plugin's handlers and function tools to ``instance`` and marks them active."""
    module_path = md.module_path or ""
    for handler in star_handlers_registry.get_handlers_by_module_name(module_path):
        raw = handler.handler.func if isinstance(handler.handler, functools.partial) else handler.handler
        handler.handler = functools.partial(raw, instance)
    for ft in _plugin_tools(module_path):
        owner = getattr(ft.handler, "__module__", None) if ft.handler is not None else None
        if ft.handler is not None and (owner == module_path or (isinstance(ft.handler, functools.partial) and ft.handler_module_path == module_path)):
            raw = ft.handler.func if isinstance(ft.handler, functools.partial) else ft.handler
            ft.handler_module_path = module_path
            ft.handler = functools.partial(raw, instance)
        path = getattr(ft, "handler_module_path", None) or ""
        if path == module_path or path.startswith(module_path + "."):
            ft.active = True
    md.activated = True
    md.star_handler_full_names = [h.handler_full_name for h in star_handlers_registry.get_handlers_by_module_name(module_path)]


async def instantiate(md: StarMetadata, plugin_dir: Path, plugin_name: str, config_values: dict | None) -> LoadedPlugin:
    """Creates the plugin object the way AstrBot does. Must run on the event loop thread."""
    _apply_metadata(md, plugin_dir, plugin_name)
    config = plugin_config(plugin_dir, plugin_name, config_values)
    context = await adapter.build_context()
    StarTools.initialize(context)
    cls = md.star_cls_type
    assert cls is not None
    author, _, name = md.plugin_id.partition("/")
    cls.name = name
    cls.author = author
    cls.plugin_id = md.plugin_id
    if config is None and _requires_config(cls):
        config = plugin_config(plugin_dir, plugin_name, config_values, required=True)
    if config is not None:
        try:
            instance = cls(context=context, config=config)
        except TypeError:
            instance = cls(context=context)
    else:
        instance = cls(context=context)
    instance.name = name
    instance.author = author
    instance.plugin_id = md.plugin_id
    md.star_cls = instance
    md.config = config
    bind(md, instance)
    return LoadedPlugin(
        name=md.name or plugin_name,
        version=md.version or "",
        package="",
        metadata=md,
        instance=instance,
        context=context,
        config=config,
    )


def import_main(
    plugin_dir: Path,
    venv_site_packages: str | None,
    data_dir: Path,
    plugin_name: str,
    preimported: tuple[str, types.ModuleType] | None = None,
) -> tuple[str, types.ModuleType]:
    """Prepares paths and the AstrBot root and imports the plugin. ``preimported`` is the package and main module already in sys.modules."""
    plugin_dir = plugin_dir.resolve()
    if preimported is None:
        prepare_paths(plugin_dir, venv_site_packages, data_dir.resolve(), plugin_name)
        return import_plugin_package(plugin_dir, plugin_name)
    enter_data_dir(plugin_dir, data_dir.resolve(), plugin_name)
    return preimported


async def load(
    plugin_dir: Path,
    venv_site_packages: str | None,
    data_dir: Path,
    plugin_name: str,
    config_values: dict | None,
    preimported: tuple[str, types.ModuleType] | None = None,
) -> LoadedPlugin:
    """Imports and instantiates the plugin. Must run on the event loop thread."""
    package, main = import_main(plugin_dir, venv_site_packages, data_dir, plugin_name, preimported)
    md = find_star(package, main)
    md.module = main
    md.root_dir_name = plugin_dir.resolve().name
    loaded = await instantiate(md, plugin_dir.resolve(), plugin_name, config_values)
    loaded.package = package
    return loaded
