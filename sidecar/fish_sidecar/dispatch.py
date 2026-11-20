"""Event matching and handler execution for one loaded plugin."""

from __future__ import annotations

import inspect
import logging
import time
import traceback
from collections import OrderedDict
from typing import Any

from astrbot import _wire
from fish_sidecar import tools
from astrbot._unsupported import UnsupportedFeature
from astrbot.core.message.components import Plain
from astrbot.core.message.message_event_result import MessageChain, MessageEventResult, ResultContentType
from astrbot.core.platform.astr_message_event import AstrMessageEvent
from astrbot.core.platform.astrbot_message import AstrBotMessage, MessageMember
from astrbot.core.platform.message_session import MessageSession
from astrbot.core.platform.message_type import MessageType
from astrbot.core.platform.platform_metadata import PlatformMetadata
from astrbot.core.provider.entities import LLMResponse, ProviderRequest
from astrbot.core.provider.provider import _context_dicts
from astrbot.core.star.filter.command import CommandFilter
from astrbot.core.star.filter.command_group import CommandGroupFilter
from astrbot.core.star.filter.event_message_type import EventMessageTypeFilter
from astrbot.core.star.filter.permission import PermissionTypeFilter
from astrbot.core.star.filter.platform_adapter_type import PlatformAdapterTypeFilter
from astrbot.core.star.filter.regex import RegexFilter
from astrbot.core.star.star_handler import EventType, StarHandlerMetadata, star_handlers_registry
from astrbot.core.utils.session_waiter import SessionWaiter, find_waiting_session

log = logging.getLogger("astrbot")

PLATFORM_ADAPTER_NAMES = {"qq_official": "qq_official", "weixin_ilink": "weixin_oc"}

HOOK_KINDS = {
    EventType.OnLLMRequestEvent: "llm_request",
    EventType.OnDecoratingResultEvent: "decorating_result",
    EventType.OnAstrBotLoadedEvent: "on_loaded",
    EventType.OnPlatformLoadedEvent: "platform_loaded",
}

INERT_EVENT_TYPES = {
    EventType.OnAgentBeginEvent: "on_agent_begin",
    EventType.OnAgentDoneEvent: "on_agent_done",
    EventType.OnUsingLLMToolEvent: "on_using_llm_tool",
    EventType.OnLLMToolRespondEvent: "on_llm_tool_respond",
    EventType.OnPluginLoadedEvent: "on_plugin_loaded",
    EventType.OnPluginUnloadedEvent: "on_plugin_unloaded",
}

PERMISSION_DENIED = "你没有权限使用这个指令"
TOOL_SENT_DIRECTLY = "工具已把内容直接发给用户"
TOOL_NO_OUTPUT = "工具执行完成，没有返回内容"


def tool_name(md: StarHandlerMetadata) -> str | None:
    """The function tool name of an ``llm_tool`` handler. Sub agents return None."""
    if md.event_type != EventType.OnCallingFuncToolEvent or "agent_name" in md.extras_configs:
        return None
    name = md.extras_configs.get("tool_name")
    return name if isinstance(name, str) and tools.TOOL_NAME.match(name) else None


def describe_error(e: BaseException) -> str:
    name = "NotImplementedError" if isinstance(e, UnsupportedFeature) else type(e).__name__
    return f"{name}: {e}"


def build_event(data: dict[str, Any]) -> AstrMessageEvent:
    """Turns a node MessageEvent into an AstrMessageEvent."""
    is_group = data.get("message_type") == "group"
    woken = bool(data.get("is_at_or_wake"))
    text = str(data.get("message_str") or "")

    msg = AstrBotMessage()
    msg.type = MessageType.GROUP_MESSAGE if is_group else MessageType.FRIEND_MESSAGE
    msg.self_id = str(data.get("self_id") or "")
    msg.session_id = str(data.get("session_id") or "")
    msg.message_id = str(data.get("message_id") or "")
    msg.group_id = str(data.get("group_id") or "") if is_group else ""
    sender = data.get("sender") or {}
    msg.sender = MessageMember(user_id=str(sender.get("user_id") or ""), nickname=sender.get("nickname"))
    msg.message = _wire.chain_from_wire(data.get("chain"))
    msg.message_str = text
    msg.raw_message = data.get("raw")
    ts = data.get("timestamp") or time.time()
    msg.timestamp = int(ts / 1000) if ts > 1e11 else int(ts)

    platform = str(data.get("platform") or "")
    umo = str(data.get("unified_msg_origin") or "")
    try:
        session = MessageSession.from_str(umo)
    except ValueError:
        session = MessageSession(platform, msg.type, msg.session_id)
    meta = PlatformMetadata(name=PLATFORM_ADAPTER_NAMES.get(platform, platform), id=session.platform_id)
    event = AstrMessageEvent(text, msg, meta, session.session_id)
    event.session = session
    event.is_at_or_wake_command = woken
    event.is_wake = woken
    event.role = "admin" if data.get("is_admin") else "member"
    return event


def bind(md: StarHandlerMetadata, instance: Any) -> Any:
    """Binds a handler defined in the plugin class to the plugin object."""
    fn = md.handler
    owner = fn.__qualname__.rpartition(".")[0]
    if owner and any(cls.__qualname__ == owner for cls in type(instance).__mro__):
        return fn.__get__(instance, type(instance))
    return fn


def handler_descriptor(md: StarHandlerMetadata) -> dict[str, Any] | None:
    """The hello entry for a handler, or None when the kind is not reported."""
    out: dict[str, Any] = {"id": md.handler_full_name, "priority": md.priority}
    if md.event_type == EventType.AdapterMessageEvent:
        filters = md.event_filters
        group = next((f for f in filters if isinstance(f, CommandGroupFilter)), None)
        command = next((f for f in filters if isinstance(f, CommandFilter)), None)
        regex = next((f for f in filters if isinstance(f, RegexFilter)), None)
        mtype = next((f for f in filters if isinstance(f, EventMessageTypeFilter)), None)
        platform = next((f for f in filters if isinstance(f, PlatformAdapterTypeFilter)), None)
        if group is not None:
            out.update(kind="command_group", pattern=group.get_complete_command_names()[0])
        elif command is not None:
            out.update(kind="command", pattern=command.get_complete_command_names()[0])
        elif regex is not None:
            out.update(kind="regex", pattern=regex.regex_str)
        elif mtype is not None:
            out.update(kind="event_message_type", pattern=mtype.event_message_type.name or str(mtype.event_message_type))
        elif platform is not None and platform.platform_type is not None:
            out.update(kind="event_message_type", pattern=f"platform:{platform.platform_type.name}")
        elif filters:
            out.update(kind="event_message_type", pattern="ALL")
        else:
            return None
        return out
    name = tool_name(md)
    if name is not None:
        out.update(kind="llm_tool", pattern=name, tool=tools.tool_schema(name, md.handler))
        return out
    kind = HOOK_KINDS.get(md.event_type)
    if kind is None:
        return None
    out["kind"] = kind
    return out


class Dispatcher:
    """Runs the plugin side of the message pipeline. ``runtime`` provides ``write`` and ``llm``."""

    def __init__(self, runtime: Any, instance: Any, plugin_name: str) -> None:
        self.runtime = runtime
        self.instance = instance
        self.plugin_name = plugin_name
        self.events: OrderedDict[str, AstrMessageEvent] = OrderedDict()
        self.max_events = 512

    def handlers(self, event_type: EventType) -> list[StarHandlerMetadata]:
        return star_handlers_registry.get_handlers_by_event_type(event_type)

    def descriptors(self) -> list[dict[str, Any]]:
        return [d for d in (handler_descriptor(md) for md in star_handlers_registry) if d is not None]

    def inert_handlers(self) -> list[str]:
        out = [
            f"{INERT_EVENT_TYPES[md.event_type]}:{md.handler_name}"
            for md in star_handlers_registry
            if md.event_type in INERT_EVENT_TYPES
        ]
        out += [
            f"llm_tool:{md.handler_name}"
            for md in star_handlers_registry
            if md.event_type == EventType.OnCallingFuncToolEvent and tool_name(md) is None
        ]
        return out

    def remember(self, event_id: str, event: AstrMessageEvent) -> None:
        self.events[event_id] = event
        self.events.move_to_end(event_id)
        while len(self.events) > self.max_events:
            self.events.popitem(last=False)

    async def call_hooks(self, event_type: EventType, *args: Any) -> None:
        """Runs hooks in priority order. A yielded result replaces the event result without sending it."""
        event = args[0] if args and isinstance(args[0], AstrMessageEvent) else None
        for md in self.handlers(event_type):
            try:
                ret = bind(md, self.instance)(*args)
                if inspect.isasyncgen(ret):
                    async for item in ret:
                        if event is not None and isinstance(item, MessageEventResult):
                            event.set_result(item)
                elif inspect.isawaitable(ret):
                    await ret
            except Exception as e:
                log.error("hook %s failed: %s\n%s", md.handler_name, describe_error(e), traceback.format_exc(limit=8))
            if event is not None and event_type != EventType.OnDecoratingResultEvent and event.is_stopped():
                break

    def match(self, event: AstrMessageEvent) -> tuple[list[tuple[StarHandlerMetadata, dict]], str | None]:
        """Returns activated handlers with parsed parameters, and an immediate reply when a
        filter rejects the message with an explanation."""
        activated: list[tuple[StarHandlerMetadata, dict]] = []
        cfg = self.instance.context.get_config() if hasattr(self.instance, "context") else {}
        for md in self.handlers(EventType.AdapterMessageEvent):
            if not md.event_filters:
                continue
            passed = True
            denied = False
            denied_loud = False
            event._extras.pop("parsed_params", None)
            for f in md.event_filters:
                try:
                    if isinstance(f, PermissionTypeFilter):
                        if not f.filter(event, cfg):
                            denied = True
                            denied_loud = denied_loud or f.raise_error
                    elif not f.filter(event, cfg):
                        passed = False
                        break
                except Exception as e:
                    event.stop_event()
                    return [], str(e)
            if not passed:
                continue
            if denied:
                if not denied_loud:
                    continue
                event.stop_event()
                return [], PERMISSION_DENIED
            if any(isinstance(f, CommandGroupFilter) for f in md.event_filters):
                continue
            activated.append((md, event._extras.pop("parsed_params", {}) or {}))
        return activated, None

    async def handle_event(self, event_id: str, data: dict[str, Any]) -> None:
        event = build_event(data)
        event._fish_event_id = event_id
        self.remember(event_id, event)
        llm_requested = False
        matched_sent = False
        try:
            waiting = find_waiting_session(event)
            if waiting is not None:
                self.runtime.write({"op": "matched", "event_id": event_id, "matched": True})
                matched_sent = True
                await SessionWaiter.trigger(waiting, event)
                event.stop_event()
            else:
                activated, reply = self.match(event)
                self.runtime.write({"op": "matched", "event_id": event_id, "matched": bool(activated or reply)})
                matched_sent = True
                if reply:
                    await event.send(MessageChain([Plain(reply)]))
                for md, params in activated:
                    llm_requested = await self.run_handler(event, md, params) or llm_requested
                    if event.is_stopped():
                        break
        except Exception as e:
            log.error("event handling failed: %s\n%s", describe_error(e), traceback.format_exc(limit=8))
            if not matched_sent:
                self.runtime.write({"op": "matched", "event_id": event_id, "matched": False})
        finally:
            stop = event.is_stopped()
            call_llm = not stop and not event._has_send_oper and not event.call_llm and not llm_requested
            self.runtime.write({"op": "done", "event_id": event_id, "stop": stop, "call_llm": call_llm})

    async def run_handler(self, event: AstrMessageEvent, md: StarHandlerMetadata, params: dict) -> bool:
        """Runs one message handler. Returns True when it asked for an LLM call."""
        requested = False
        try:
            ret = bind(md, self.instance)(event, **params)
            if inspect.isasyncgen(ret):
                async for item in ret:
                    requested = await self.handle_item(event, item) or requested
            elif inspect.isawaitable(ret):
                requested = await self.handle_item(event, await ret)
            else:
                requested = await self.handle_item(event, ret)
        except Exception as e:
            text = traceback.format_exc(limit=8)
            log.error("handler %s failed: %s\n%s", md.handler_name, describe_error(e), text)
            await self.call_hooks(EventType.OnPluginErrorEvent, event, self.plugin_name, md.handler_name, e, text)
        await self.respond(event)
        return requested

    async def handle_item(self, event: AstrMessageEvent, item: Any) -> bool:
        """Processes one value a handler yielded or returned."""
        if isinstance(item, MessageEventResult):
            event.set_result(item)
        elif isinstance(item, ProviderRequest):
            await self.respond(event)
            await self.run_llm_request(event, item)
            return True
        elif isinstance(item, MessageChain):
            await event.send(item)
        elif isinstance(item, str) and item:
            event.set_result(MessageEventResult().message(item))
        await self.respond(event)
        return False

    async def respond(self, event: AstrMessageEvent) -> None:
        """Decorates and sends the pending result, then clears it and keeps the stop flag."""
        result = event.get_result()
        if result is None:
            return
        if result.async_stream is not None:
            parts: list = []
            async for chunk in result.async_stream:
                if isinstance(chunk, MessageChain):
                    parts.extend(chunk.chain)
            result.async_stream = None
            result.chain.extend(parts)
        if result.chain:
            await self.call_hooks(EventType.OnDecoratingResultEvent, event)
            result = event.get_result() or result
        stopped = result.is_stopped()
        if result.chain:
            await event.send(result)
            await self.call_hooks(EventType.OnAfterMessageSentEvent, event)
        event.clear_result()
        if stopped:
            event._force_stopped = True

    async def run_llm_request(self, event: AstrMessageEvent, req: ProviderRequest) -> None:
        await self.call_hooks(EventType.OnWaitingLLMRequestEvent, event)
        await self.call_hooks(EventType.OnLLMRequestEvent, event, req)
        if event.is_stopped():
            return
        try:
            text = await self.runtime.llm(req.full_prompt(), req.system_prompt or None, _context_dicts(req.contexts), req.image_urls or None)
        except Exception as e:
            log.error("LLM request failed: %s", describe_error(e))
            return
        response = LLMResponse(role="assistant", completion_text=text)
        await self.call_hooks(EventType.OnLLMResponseEvent, event, response)
        result = MessageEventResult(chain=[Plain(response.completion_text)])
        result.set_result_content_type(ResultContentType.LLM_RESULT)
        event.set_result(result)
        await self.respond(event)

    async def handle_llm_hook(self, event_id: str, request: dict[str, Any]) -> dict[str, Any]:
        """Runs on_llm_request hooks for a node default LLM call.

        shortcut: extra content parts are folded into the prompt as text and images are
        dropped, extend llm_hook_result with parts when the node supports multimodal hooks.
        """
        req = ProviderRequest(
            prompt=request.get("prompt"),
            system_prompt=request.get("system_prompt") or "",
            contexts=list(request.get("contexts") or []),
            image_urls=list(request.get("image_urls") or []),
        )
        event = self.events.get(event_id)
        if event is not None:
            await self.call_hooks(EventType.OnLLMRequestEvent, event, req)
        out: dict[str, Any] = {
            "prompt": req.full_prompt(),
            "system_prompt": req.system_prompt,
            "contexts": _context_dicts(req.contexts) or [],
        }
        if req.image_urls:
            out["image_urls"] = req.image_urls
        return out

    async def handle_tool_call(self, event_id: str, data: dict[str, Any], name: str, arguments: Any) -> dict[str, Any]:
        """Runs an ``llm_tool`` handler and returns ``{"text", "sent"}`` or ``{"error"}``.

        Returned and yielded strings form the tool result. Yielded or returned message
        results are sent to the user directly, as in AstrBot.
        """
        md = next((m for m in star_handlers_registry if tool_name(m) == name), None)
        if md is None:
            return {"error": f"没有名为 {name} 的工具"}
        event = self.events.get(event_id)
        if event is None:
            event = build_event(data)
            event._fish_event_id = event_id
            self.remember(event_id, event)
        fn = bind(md, self.instance)
        kwargs = tools.call_kwargs(fn, arguments if isinstance(arguments, dict) else {})
        texts: list[str] = []
        sent_before = event._has_send_oper
        event._has_send_oper = False

        async def take(item: Any) -> None:
            if item is None:
                return
            if isinstance(item, (MessageEventResult, MessageChain)):
                if isinstance(item, MessageEventResult):
                    event.set_result(item)
                else:
                    await event.send(item)
                await self.respond(event)
            else:
                texts.append(str(item))

        try:
            ret = fn(event, **kwargs)
            if inspect.isasyncgen(ret):
                async for item in ret:
                    await take(item)
            elif inspect.isawaitable(ret):
                await take(await ret)
            else:
                await take(ret)
        except Exception as e:
            log.error("tool %s failed: %s\n%s", name, describe_error(e), traceback.format_exc(limit=8))
            return {"error": f"工具执行出错：{describe_error(e)}", "sent": event._has_send_oper}
        finally:
            sent = event._has_send_oper
            event._has_send_oper = sent_before or sent
        text = "\n".join(t for t in texts if t)
        if not text:
            text = TOOL_SENT_DIRECTLY if sent else TOOL_NO_OUTPUT
        return {"text": text, "sent": sent}
