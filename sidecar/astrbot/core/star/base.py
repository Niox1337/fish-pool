"""Base class of every plugin."""

from __future__ import annotations

import logging
import re
from typing import TYPE_CHECKING, Any

from astrbot._unsupported import unsupported
from astrbot.core.star.star import StarMetadata, star_map, star_registry
from astrbot.core.utils.shared_preferences import sp

if TYPE_CHECKING:
    from astrbot.core.star.context import Context


class CommandTokens:
    def __init__(self) -> None:
        self.tokens: list[str] = []
        self.len = 0

    def get(self, idx: int) -> str | None:
        return self.tokens[idx].strip() if idx < self.len else None


class Star:
    """Plugins subclass Star. Defining a subclass registers it for the module it lives in."""

    author: str
    name: str
    plugin_id: str
    context: Context

    def __init__(self, context: Context, config: dict | None = None) -> None:
        self.context = context
        md = star_map.get(type(self).__module__)
        plugin = (md.name if md else None) or getattr(self, "name", None)
        try:
            self.logger = logging.getLogger(f"astrbot.plugin.{plugin}" if plugin else "astrbot")
        except AttributeError:
            pass

    def __init_subclass__(cls, **kwargs: Any) -> None:
        super().__init_subclass__(**kwargs)
        md = star_map.get(cls.__module__)
        if md is None:
            md = StarMetadata(star_cls_type=cls, module_path=cls.__module__)
            star_map[cls.__module__] = md
            star_registry.append(md)
        else:
            md.star_cls_type = cls
            md.module_path = cls.__module__

    async def initialize(self) -> None:
        """Runs once after the plugin object is created."""

    async def terminate(self) -> None:
        """Runs once before the plugin process exits."""

    async def text_to_image(self, text: str, return_url: bool = True, template_name: str | None = None, umo: str | None = None) -> str:
        unsupported("Star.text_to_image")

    async def html_render(self, tmpl: str, data: dict, return_url: bool = True, options: dict | None = None, umo: str | None = None) -> str:
        unsupported("Star.html_render")

    def _kv_scope(self) -> str:
        return getattr(self, "plugin_id", None) or getattr(self, "name", None) or type(self).__module__

    async def put_kv_data(self, key: str, value: Any) -> None:
        await sp.put_async("plugin", self._kv_scope(), key, value)

    async def get_kv_data(self, key: str, default: Any = None) -> Any:
        return await sp.get_async("plugin", self._kv_scope(), key, default)

    async def delete_kv_data(self, key: str) -> None:
        await sp.remove_async("plugin", self._kv_scope(), key)

    def parse_commands(self, message: str) -> CommandTokens:
        tokens = CommandTokens()
        tokens.tokens = re.split(r"\s+", message)
        tokens.len = len(tokens.tokens)
        return tokens

    def regex_match(self, message: str, command: str) -> bool:
        return re.search(command, message, re.MULTILINE) is not None
