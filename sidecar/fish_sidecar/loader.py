"""Loads one AstrBot plugin directory into the current process."""

from __future__ import annotations

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

from astrbot.core.config.astrbot_config import AstrBotConfig
from astrbot.core.star.base import Star
from astrbot.core.star.context import Context
from astrbot.core.star.star import StarMetadata, star_registry
from astrbot.core.star.star_tools import StarTools

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
    config: AstrBotConfig


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


def find_star(package: str, main_module: str) -> StarMetadata:
    candidates = [
        md
        for md in star_registry
        if md.star_cls_type is not None and md.module_path and (md.module_path == package or md.module_path.startswith(package + "."))
    ]
    if not candidates:
        raise LookupError("no Star subclass found in the plugin")
    in_main = [md for md in candidates if md.module_path == main_module] or candidates
    registered = [md for md in in_main if md.registered]
    return (registered or in_main)[-1]


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


def _accepts_config(cls: type) -> bool:
    try:
        params = list(inspect.signature(cls.__init__).parameters.values())[1:]
    except (TypeError, ValueError):
        return True
    if any(p.kind in (p.VAR_POSITIONAL, p.VAR_KEYWORD) for p in params):
        return True
    positional = [p for p in params if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)]
    return len(positional) >= 2 or any(p.name == "config" for p in params)


def instantiate(md: StarMetadata, plugin_dir: Path, plugin_name: str, config_values: dict | None) -> LoadedPlugin:
    meta = read_metadata(plugin_dir)
    md.name = md.name or meta.get("name") or plugin_name
    md.author = md.author or meta.get("author")
    md.desc = md.desc or meta.get("desc") or meta.get("description")
    md.version = md.version or (str(meta["version"]) if meta.get("version") is not None else None)
    md.repo = md.repo or meta.get("repo")
    md.display_name = md.display_name or meta.get("display_name")
    config = AstrBotConfig(
        config_path=str(Path.cwd() / "config" / f"{plugin_name}_config.json"),
        schema=read_schema(plugin_dir),
        values=config_values,
    )
    context = Context()
    StarTools.initialize(context)
    cls = md.star_cls_type
    assert cls is not None
    cls.name = md.name
    cls.author = md.author or "unknown"
    cls.plugin_id = md.plugin_id
    instance = cls(context, config) if _accepts_config(cls) else cls(context)
    md.star_cls = instance
    md.config = config
    return LoadedPlugin(
        name=md.name or plugin_name,
        version=md.version or "",
        package="",
        metadata=md,
        instance=instance,
        context=context,
        config=config,
    )


def load(
    plugin_dir: Path,
    venv_site_packages: str | None,
    data_dir: Path,
    plugin_name: str,
    config_values: dict | None,
    preimported: tuple[str, types.ModuleType] | None = None,
) -> LoadedPlugin:
    """Imports and instantiates the plugin. ``preimported`` is the package and main module already in sys.modules."""
    plugin_dir = plugin_dir.resolve()
    if preimported is None:
        prepare_paths(plugin_dir, venv_site_packages, data_dir.resolve(), plugin_name)
        package, main = import_plugin_package(plugin_dir, plugin_name)
    else:
        enter_data_dir(plugin_dir, data_dir.resolve(), plugin_name)
        package, main = preimported
    md = find_star(package, main.__name__)
    md.module = main
    md.root_dir_name = plugin_dir.name
    loaded = instantiate(md, plugin_dir, plugin_name, config_values)
    loaded.package = package
    return loaded
