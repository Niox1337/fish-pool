"""Registry of handler metadata collected by the plugin decorators."""

from __future__ import annotations

import enum
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any


class EventType(enum.Enum):
    OnAstrBotLoadedEvent = enum.auto()
    OnPlatformLoadedEvent = enum.auto()
    AdapterMessageEvent = enum.auto()
    OnWaitingLLMRequestEvent = enum.auto()
    OnLLMRequestEvent = enum.auto()
    OnLLMResponseEvent = enum.auto()
    OnAgentBeginEvent = enum.auto()
    OnAgentDoneEvent = enum.auto()
    OnDecoratingResultEvent = enum.auto()
    OnCallingFuncToolEvent = enum.auto()
    OnUsingLLMToolEvent = enum.auto()
    OnLLMToolRespondEvent = enum.auto()
    OnAfterMessageSentEvent = enum.auto()
    OnPluginErrorEvent = enum.auto()
    OnPluginLoadedEvent = enum.auto()
    OnPluginUnloadedEvent = enum.auto()


@dataclass
class StarHandlerMetadata:
    event_type: EventType
    handler_full_name: str
    handler_name: str
    handler_module_path: str
    handler: Callable[..., Any]
    event_filters: list = field(default_factory=list)
    desc: str = ""
    extras_configs: dict = field(default_factory=dict)
    enabled: bool = True

    @property
    def priority(self) -> int:
        return int(self.extras_configs.get("priority", 0) or 0)

    def __lt__(self, other: StarHandlerMetadata) -> bool:
        return self.priority < other.priority


class StarHandlerRegistry:
    def __init__(self) -> None:
        self.star_handlers_map: dict[str, StarHandlerMetadata] = {}
        self._handlers: list[StarHandlerMetadata] = []

    def append(self, handler: StarHandlerMetadata) -> None:
        handler.extras_configs.setdefault("priority", 0)
        self.star_handlers_map[handler.handler_full_name] = handler
        self._handlers.append(handler)
        self._handlers.sort(key=lambda h: -h.priority)

    def get_handlers_by_event_type(
        self, event_type: EventType, only_activated: bool = True, plugins_name: list[str] | None = None
    ) -> list[StarHandlerMetadata]:
        return [h for h in self._handlers if h.event_type == event_type and (h.enabled or not only_activated)]

    def get_handler_by_full_name(self, full_name: str) -> StarHandlerMetadata | None:
        return self.star_handlers_map.get(full_name)

    def get_handlers_by_module_name(self, module_name: str) -> list[StarHandlerMetadata]:
        return [h for h in self._handlers if h.handler_module_path == module_name]

    def remove(self, handler: StarHandlerMetadata) -> None:
        self.star_handlers_map.pop(handler.handler_full_name, None)
        self._handlers = [h for h in self._handlers if h is not handler]

    def clear(self) -> None:
        self.star_handlers_map.clear()
        self._handlers.clear()

    def __iter__(self):
        return iter(self._handlers)

    def __len__(self) -> int:
        return len(self._handlers)


star_handlers_registry = StarHandlerRegistry()
