"""Handler decorators. Each one records metadata in ``star_handlers_registry``."""

from __future__ import annotations

import logging
import re
from collections.abc import Callable
from typing import Any

from astrbot.core.star.filter.command import CommandFilter
from astrbot.core.star.filter.command_group import CommandGroupFilter
from astrbot.core.star.filter.custom_filter import CustomFilter, CustomFilterAnd, CustomFilterOr
from astrbot.core.star.filter.event_message_type import EventMessageType, EventMessageTypeFilter
from astrbot.core.star.filter.interaction import ButtonActionFilter, UIActionFilter
from astrbot.core.star.filter.permission import PermissionType, PermissionTypeFilter
from astrbot.core.star.filter.platform_adapter_type import PlatformAdapterType, PlatformAdapterTypeFilter
from astrbot.core.star.filter.regex import RegexFilter
from astrbot.core.star.star_handler import EventType, StarHandlerMetadata, star_handlers_registry

log = logging.getLogger("astrbot")


def get_handler_full_name(handler: Callable[..., Any]) -> str:
    return f"{handler.__module__}_{getattr(handler, '__qualname__', handler.__name__)}"


def get_handler_or_create(handler: Callable[..., Any], event_type: EventType, dont_add: bool = False, **kwargs: Any) -> StarHandlerMetadata:
    full_name = get_handler_full_name(handler)
    existing = star_handlers_registry.get_handler_by_full_name(full_name)
    if existing is not None:
        if "priority" in kwargs:
            existing.extras_configs["priority"] = kwargs["priority"]
            star_handlers_registry._handlers.sort(key=lambda h: -h.priority)
        return existing
    md = StarHandlerMetadata(
        event_type=event_type,
        handler_full_name=full_name,
        handler_name=handler.__name__,
        handler_module_path=handler.__module__,
        handler=handler,
        event_filters=[],
        desc=(handler.__doc__ or "").strip(),
    )
    if "desc" in kwargs:
        md.desc = kwargs.pop("desc")
    md.extras_configs = dict(kwargs)
    if not dont_add:
        star_handlers_registry.append(md)
    return md


def _message_handler(filter_factory: Callable[[], Any], **kwargs: Any):
    def decorator(fn):
        md = get_handler_or_create(fn, EventType.AdapterMessageEvent, **kwargs)
        md.event_filters.append(filter_factory())
        return fn

    return decorator


def register_command(command_name: Any = None, sub_command: str | None = None, alias: set | None = None, **kwargs: Any):
    """Registers a command, or a sub command when called on a command group."""
    new_filter: CommandFilter | None = None
    if isinstance(command_name, RegisteringCommandable):
        if sub_command is None:
            log.warning("sub command registration on %s has no name", command_name.parent_group.group_name)
        else:
            parent = command_name.parent_group
            new_filter = CommandFilter(sub_command, alias, None, parent_command_names=parent.get_complete_command_names())
            parent.add_sub_command_filter(new_filter)
            kwargs["sub_command"] = True
    elif command_name is None:
        log.warning("command registration has no command name")
    else:
        new_filter = CommandFilter(command_name, alias)

    def decorator(fn):
        md = get_handler_or_create(fn, EventType.AdapterMessageEvent, **kwargs)
        if new_filter is not None:
            new_filter.init_handler_md(md)
            md.event_filters.append(new_filter)
        return fn

    return decorator


def register_command_group(command_group_name: Any = None, sub_command: str | None = None, alias: set | None = None, **kwargs: Any):
    """Registers a command group. The decorated function becomes a group object with ``.command`` and ``.group``."""
    if isinstance(command_group_name, RegisteringCommandable):
        if sub_command is None:
            raise ValueError("a nested command group needs a name")
        parent = command_group_name.parent_group
        new_group = CommandGroupFilter(sub_command, alias, parent_group=parent)
        parent.add_sub_command_filter(new_group)
    elif command_group_name is None:
        raise ValueError("a command group needs a name")
    else:
        new_group = CommandGroupFilter(command_group_name, alias)

    def decorator(fn):
        md = get_handler_or_create(fn, EventType.AdapterMessageEvent, **kwargs)
        md.event_filters.append(new_group)
        return RegisteringCommandable(new_group)

    return decorator


def register_custom_filter(custom_type_filter: Any, *args: Any, **kwargs: Any):
    """Adds a CustomFilter subclass or instance to a handler, a sub command or a group."""
    parent: RegisteringCommandable | None = None
    if isinstance(custom_type_filter, RegisteringCommandable):
        parent = custom_type_filter
        custom = args[0]
        raise_error = args[1] if len(args) > 1 else True
    else:
        custom = custom_type_filter
        raise_error = args[0] if args else True
    if not isinstance(custom, (CustomFilter, CustomFilterAnd, CustomFilterOr)):
        custom = custom(raise_error)

    def decorator(target):
        if isinstance(target, RegisteringCommandable):
            target.parent_group.add_custom_filter(custom)
            return target
        md = get_handler_or_create(target, EventType.AdapterMessageEvent, **kwargs)
        if parent is not None:
            for sub in parent.parent_group.sub_command_filters:
                if isinstance(sub, CommandFilter) and sub.handler_md is md:
                    sub.add_custom_filter(custom)
        else:
            md.event_filters.append(custom)
        return target

    return decorator


class RegisteringCommandable:
    """Returned by ``command_group``. Its methods register children of the group."""

    group = register_command_group
    command = register_command
    custom_filter = register_custom_filter

    def __init__(self, parent_group: CommandGroupFilter) -> None:
        self.parent_group = parent_group


def register_event_message_type(event_message_type: EventMessageType, **kwargs: Any):
    return _message_handler(lambda: EventMessageTypeFilter(event_message_type), **kwargs)


def register_platform_adapter_type(platform_adapter_type: PlatformAdapterType | str, **kwargs: Any):
    return _message_handler(lambda: PlatformAdapterTypeFilter(platform_adapter_type), **kwargs)


def register_regex(regex: str | re.Pattern, **kwargs: Any):
    return _message_handler(lambda: RegexFilter(regex), **kwargs)


def register_permission_type(permission_type: PermissionType, raise_error: bool = True, **kwargs: Any):
    return _message_handler(lambda: PermissionTypeFilter(permission_type, raise_error), **kwargs)


def register_button_action(action_id: str | None = None, **kwargs: Any):
    return _message_handler(lambda: ButtonActionFilter(action_id), **kwargs)


def register_ui_action(action_id: str | None = None, **kwargs: Any):
    return _message_handler(lambda: UIActionFilter(action_id), **kwargs)


def _hook(event_type: EventType):
    def register(**kwargs: Any):
        def decorator(fn):
            get_handler_or_create(fn, event_type, **kwargs)
            return fn

        return decorator

    return register


register_on_astrbot_loaded = _hook(EventType.OnAstrBotLoadedEvent)
register_on_platform_loaded = _hook(EventType.OnPlatformLoadedEvent)
register_on_plugin_error = _hook(EventType.OnPluginErrorEvent)
register_on_plugin_loaded = _hook(EventType.OnPluginLoadedEvent)
register_on_plugin_unloaded = _hook(EventType.OnPluginUnloadedEvent)
register_on_waiting_llm_request = _hook(EventType.OnWaitingLLMRequestEvent)
register_on_llm_request = _hook(EventType.OnLLMRequestEvent)
register_on_llm_response = _hook(EventType.OnLLMResponseEvent)
register_on_agent_begin = _hook(EventType.OnAgentBeginEvent)
register_on_agent_done = _hook(EventType.OnAgentDoneEvent)
register_on_using_llm_tool = _hook(EventType.OnUsingLLMToolEvent)
register_on_llm_tool_respond = _hook(EventType.OnLLMToolRespondEvent)
register_on_decorating_result = _hook(EventType.OnDecoratingResultEvent)
register_after_message_sent = _hook(EventType.OnAfterMessageSentEvent)


def register_llm_tool(name: str | None = None, **kwargs: Any):
    """Registers a function tool. The node offers it to the default LLM reply."""

    def decorator(fn):
        md = get_handler_or_create(fn, EventType.OnCallingFuncToolEvent)
        md.extras_configs["tool_name"] = name or fn.__name__
        return fn

    return decorator


class RegisteringAgent:
    def __init__(self, agent: Any = None) -> None:
        self._agent = agent

    def llm_tool(self, *args: Any, **kwargs: Any):
        return register_llm_tool(*args, **kwargs)


def register_agent(name: str, instruction: str, tools: list | None = None, run_hooks: Any = None):
    """Records a sub agent. It is never invoked."""

    def decorator(fn):
        md = get_handler_or_create(fn, EventType.OnCallingFuncToolEvent)
        md.extras_configs["agent_name"] = name
        return RegisteringAgent(None)

    return decorator
