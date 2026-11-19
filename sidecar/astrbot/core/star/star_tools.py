"""Class level helpers for plugins."""

from __future__ import annotations

import inspect
from pathlib import Path
from typing import Any

from astrbot import _fish
from astrbot._unsupported import unsupported
from astrbot.core.message.message_event_result import MessageChain
from astrbot.core.star.star import star_map
from astrbot.core.utils.astrbot_path import get_astrbot_plugin_data_path


class StarTools:
    _context: Any = None

    @classmethod
    def initialize(cls, context: Any) -> None:
        cls._context = context

    @classmethod
    async def send_message(cls, session: Any, message_chain: MessageChain) -> bool:
        if cls._context is None:
            raise RuntimeError("StarTools is not initialized")
        return await cls._context.send_message(session, message_chain)

    @classmethod
    async def send_message_by_id(cls, *args: Any, **kwargs: Any) -> None:
        unsupported("StarTools.send_message_by_id")

    @classmethod
    async def create_message(cls, *args: Any, **kwargs: Any) -> Any:
        unsupported("StarTools.create_message")

    @classmethod
    async def create_event(cls, *args: Any, **kwargs: Any) -> Any:
        unsupported("StarTools.create_event")

    @classmethod
    def activate_llm_tool(cls, name: str) -> bool:
        return False

    @classmethod
    async def activate_llm_tool_async(cls, name: str) -> bool:
        return False

    @classmethod
    def deactivate_llm_tool(cls, name: str) -> bool:
        return False

    @classmethod
    async def deactivate_llm_tool_async(cls, name: str) -> bool:
        return False

    @classmethod
    def register_llm_tool(cls, *args: Any, **kwargs: Any) -> None:
        return None

    @classmethod
    def unregister_llm_tool(cls, name: str) -> None:
        return None

    @classmethod
    def get_data_dir(cls, plugin_name: str | None = None) -> Path:
        """Returns ``<data_dir>/plugin_data/<plugin_name>`` and creates it."""
        if not plugin_name:
            plugin_name = _fish.plugin_name()
        if not plugin_name:
            frame = inspect.currentframe()
            module = inspect.getmodule(frame.f_back) if frame and frame.f_back else None
            md = star_map.get(module.__name__) if module else None
            plugin_name = md.name if md else None
        if not plugin_name:
            raise ValueError("Unable to resolve plugin name")
        path = Path(get_astrbot_plugin_data_path()) / plugin_name
        path.mkdir(parents=True, exist_ok=True)
        return path.resolve()
