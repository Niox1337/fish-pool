"""Directory helpers. Every writable path lives inside the plugin process data directory."""

from __future__ import annotations

from pathlib import Path

from astrbot import _fish


def _ensure(path: Path) -> str:
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError:
        pass
    return str(path)


def get_astrbot_root() -> str:
    return str(_fish.data_dir())


def get_astrbot_path() -> str:
    return str(Path(__file__).resolve().parents[2])


def get_astrbot_data_path() -> str:
    return str(_fish.data_dir())


def get_astrbot_config_path() -> str:
    return _ensure(_fish.data_dir() / "config")


def get_astrbot_plugin_path() -> str:
    """Directory that contains a ``<plugin_name>`` entry pointing at the plugin source."""
    return _ensure(_fish.data_dir() / "plugins")


def get_astrbot_builtin_plugin_path() -> str:
    return _ensure(_fish.data_dir() / "builtin_stars")


def get_astrbot_plugin_data_path() -> str:
    return _ensure(_fish.data_dir() / "plugin_data")


def get_astrbot_t2i_templates_path() -> str:
    return _ensure(_fish.data_dir() / "t2i_templates")


def get_astrbot_webchat_path() -> str:
    return _ensure(_fish.data_dir() / "webchat")


def get_astrbot_temp_path() -> str:
    return _ensure(_fish.data_dir() / "temp")


def get_astrbot_skills_path() -> str:
    return _ensure(_fish.data_dir() / "skills")


def get_astrbot_workspaces_path() -> str:
    return _ensure(_fish.data_dir() / "workspaces")


def get_astrbot_system_tmp_path() -> str:
    return get_astrbot_temp_path()


def get_astrbot_site_packages_path() -> str:
    return _ensure(_fish.data_dir() / "site-packages")


def get_astrbot_knowledge_base_path() -> str:
    return _ensure(_fish.data_dir() / "knowledge_base")


def get_astrbot_backups_path() -> str:
    return _ensure(_fish.data_dir() / "backups")


__all__ = [name for name in dir() if name.startswith("get_astrbot_")]
