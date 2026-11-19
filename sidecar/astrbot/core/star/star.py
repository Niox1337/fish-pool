"""Plugin metadata registry, keyed by the module that defines the plugin class."""

from __future__ import annotations

from dataclasses import dataclass, field
from types import ModuleType
from typing import Any


@dataclass
class StarMetadata:
    name: str | None = None
    author: str | None = None
    desc: str | None = None
    short_desc: str | None = None
    version: str | None = None
    repo: str | None = None
    star_cls_type: type | None = None
    module_path: str | None = None
    star_cls: Any = None
    module: ModuleType | None = None
    root_dir_name: str | None = None
    reserved: bool = False
    activated: bool = True
    config: Any = None
    star_handler_full_names: list[str] = field(default_factory=list)
    display_name: str | None = None
    logo_path: str | None = None
    support_platforms: list[str] = field(default_factory=list)
    astrbot_version: str | None = None
    registered: bool = False

    @property
    def plugin_id(self) -> str:
        name = (self.name or "unknown").lower().replace("/", "_")
        author = (self.author or "unknown").lower().replace("/", "_")
        return f"{author}/{name}"


star_registry: list[StarMetadata] = []
star_map: dict[str, StarMetadata] = {}
