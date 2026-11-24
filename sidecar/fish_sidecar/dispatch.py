"""Event matching and handler execution for one loaded plugin."""

from __future__ import annotations

import inspect
import logging
import traceback
from collections import OrderedDict
from typing import Any

from mcp.types import CallToolResult, EmbeddedResource, ImageContent, TextContent

from astrbot.core.agent.message import Message
from astrbot.core.agent.run_context import ContextWrapper
from astrbot.core.agent.tool import FunctionTool
from astrbot.core.astr_agent_context import AstrAgentContext
from astrbot.core.message.components import Plain
from astrbot.core.message.message_event_result import MessageChain, MessageEventResult, ResultContentType
from astrbot.core.platform.astr_message_event import AstrMessageEvent
from astrbot.core.provider.entities import LLMResponse, ProviderRequest
from astrbot.core.star.filter.command import CommandFilter
from astrbot.core.star.filter.command_group import CommandGroupFilter
from astrbot.core.star.filter.event_message_type import EventMessageTypeFilter
from astrbot.core.star.filter.permission import PermissionTypeFilter
from astrbot.core.star.filter.platform_adapter_type import PlatformAdapterTypeFilter
from astrbot.core.star.filter.regex import RegexFilter
from astrbot.core.star.star import StarMetadata, star_map
from astrbot.core.star.star_handler import EventType, StarHandlerMetadata, star_handlers_registry
from astrbot.core.utils.session_waiter import FILTERS, USER_SESSIONS, SessionWaiter

from fish_sidecar import adapter, tools, wire
from fish_sidecar.adapter import build_event

log = logging.getLogger("astrbot")

HOOK_KINDS = {
    EventType.OnWaitingLLMRequestEvent: "llm_request",
    EventType.OnLLMRequestEvent: "llm_request",
    EventType.OnLLMResponseEvent: "llm_response",
    EventType.OnAgentBeginEvent: "agent_begin",
    EventType.OnAgentDoneEvent: "agent_done",
    EventType.OnUsingLLMToolEvent: "using_llm_tool",
    EventType.OnLLMToolRespondEvent: "llm_tool_respond",
    EventType.OnDecoratingResultEvent: "decorating_result",
    EventType.OnAfterMessageSentEvent: "after_message_sent",
    EventType.OnPluginErrorEvent: "plugin_error",
    EventType.OnPluginLoadedEvent: "plugin_loaded",
    EventType.OnPluginUnloadedEvent: "plugin_unloaded",
    EventType.OnAstrBotLoadedEvent: "on_loaded",
    EventType.OnPlatformLoadedEvent: "platform_loaded",
}

# Handler kinds whose hooks the node runs with a message event, so the event is kept for them.
EVENT_HOOK_KINDS = frozenset(HOOK_KINDS.values()) - {"plugin_loaded", "plugin_unloaded", "on_loaded", "platform_loaded"}

# Time the node has to decorate and send one result, or to run one plugin LLM request.
DELIVER_TIMEOUT_S = 120
AGENT_TIMEOUT_S = 600

PERMISSION_DENIED = "你没有权限使用这个指令"
TOOL_SENT_DIRECTLY = "工具已把内容直接发给用户"
TOOL_NO_OUTPUT = "工具执行完成，没有返回内容"


def describe_error(e: BaseException) -> str:
    return f"{type(e).__name__}: {e}"


def priority(md: StarHandlerMetadata) -> int:
    value = md.extras_configs.get("priority", 0)
    return value if isinstance(value, int) else 0


def find_waiting_session(event: AstrMessageEvent) -> str | None:
    """The session key of a ``session_waiter`` waiting for this event."""
    for session_filter in tuple(FILTERS):
        key = session_filter.filter(event)
        session = USER_SESSIONS.get(key)
        if session is not None and not session.session_controller.future.done():
            return key
    return None


def handler_descriptor(md: StarHandlerMetadata) -> dict[str, Any] | None:
    """The hello entry for a handler, or None when the kind is not reported."""
    out: dict[str, Any] = {"id": f"{md.handler_module_path}.{md.handler_name}", "priority": priority(md)}
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
            out.update(
                kind="event_message_type", pattern=mtype.event_message_type.name or str(mtype.event_message_type)
            )
        elif platform is not None and platform.platform_type is not None:
            out.update(kind="event_message_type", pattern=f"platform:{platform.platform_type.name}")
        elif filters:
            out.update(kind="event_message_type", pattern="ALL")
        else:
            return None
        return out
    kind = HOOK_KINDS.get(md.event_type)
    if kind is None:
        return None
    out["kind"] = kind
    return out


def agent_messages(items: Any) -> list[Message]:
    """AstrBot agent messages from the node's OpenAI style message list. Invalid entries are skipped."""
    out: list[Message] = []
    for item in items if isinstance(items, list) else []:
        try:
            out.append(Message.model_validate(item))
        except Exception:
            continue
    return out


def tool_images(value: Any) -> list[str]:
    """Data URLs of the images in an MCP ``CallToolResult``."""
    out: list[str] = []
    for item in getattr(value, "content", None) or []:
        if isinstance(item, ImageContent):
            out.append(f"data:{item.mimeType or 'image/png'};base64,{item.data}")
        elif isinstance(item, EmbeddedResource):
            blob = getattr(item.resource, "blob", None)
            mime = getattr(item.resource, "mimeType", None) or ""
            if blob and mime.startswith("image/"):
                out.append(f"data:{mime};base64,{blob}")
    return out


class Dispatcher:
    """Runs the plugin side of the message pipeline. ``runtime`` provides ``write`` and ``llm``.

    ``module_prefix`` limits handlers and tools to one plugin package when the
    process holds several plugins. None uses every registered handler.
    """

    def __init__(self, runtime: Any, instance: Any, plugin_name: str, module_prefix: str | None = None) -> None:
        self.runtime = runtime
        self.instance = instance
        self.plugin_name = plugin_name
        self.module_prefix = module_prefix
        self.events: OrderedDict[str, AstrMessageEvent] = OrderedDict()
        # Set from the hello handlers. Command-only plugins do not keep events after handling them.
        self.keep_events = False
        # Agent run contexts by event id, kept from ``agent_begin`` to ``agent_done``.
        self.agent_runs: OrderedDict[str, ContextWrapper] = OrderedDict()
        manager = getattr(getattr(instance, "context", None), "platform_manager", None)
        self.platforms = manager if isinstance(manager, adapter.FishPlatformManager) else None
        self.max_events = 512

    def owns(self, md: StarHandlerMetadata) -> bool:
        prefix = self.module_prefix
        if prefix is None:
            return True
        path = md.handler_module_path or ""
        return path == prefix or path.startswith(prefix + ".")

    def registry(self) -> list[StarHandlerMetadata]:
        return [md for md in star_handlers_registry if self.owns(md)]

    def handlers(self, event_type: EventType) -> list[StarHandlerMetadata]:
        return [md for md in star_handlers_registry.get_handlers_by_event_type(event_type) if self.owns(md)]

    def descriptors(self) -> list[dict[str, Any]]:
        out = [d for d in (handler_descriptor(md) for md in self.registry()) if d is not None]
        for tool in tools.plugin_tools(self.module_prefix):
            out.append(
                {
                    "id": f"tool:{tool.name}",
                    "priority": 0,
                    "kind": "llm_tool",
                    "pattern": tool.name,
                    "tool": tools.tool_schema(tool),
                }
            )
        return out

    def wake_commands(self) -> list[str] | None:
        """Returns names the node can safely match before waking a command-only plugin."""
        if tools.plugin_tools(self.module_prefix):
            return None
        commands: set[str] = set()
        for md in self.registry():
            filters = md.event_filters
            if md.event_type != EventType.AdapterMessageEvent or len(filters) != 1:
                return None
            command = filters[0]
            if type(command) is not CommandFilter or command.custom_filter_list:
                return None
            for name in command.get_complete_command_names():
                if not isinstance(name, str) or not name or any(char.isspace() for char in name):
                    return None
                commands.add(name)
        return sorted(commands) or None

    def inert_handlers(self) -> list[str]:
        return [f"llm_tool:{name}" for name in tools.inactive_tools(self.module_prefix)]

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
                ret = md.handler(*args)
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

    def sync_admins(self, admins: Any) -> None:
        """Sets AstrBot's global ``admins_id`` to the bot admins carried by an event."""
        if not isinstance(admins, list):
            return
        manager = getattr(getattr(self.instance, "context", None), "astrbot_config_mgr", None)
        config = getattr(manager, "default_conf", None)
        if not isinstance(config, dict):
            return
        ids = [str(a) for a in admins]
        if config.get("admins_id") != ids:
            config["admins_id"] = ids

    async def handle_event(self, event_id: str, data: dict[str, Any]) -> None:
        self.sync_admins(data.get("admins", []))
        event = build_event(data, event_id)
        if self.keep_events:
            self.remember(event_id, event)
        if self.platforms is not None:
            self.platforms.remember(event.platform_meta)
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
            ret = md.handler(event, **params)
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
            self.runtime.write(
                {
                    "op": "plugin_error",
                    "event_id": getattr(event, "fish_event_id", None),
                    "handler": md.handler_name,
                    "error": describe_error(e),
                    "traceback": text,
                }
            )
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
        """Hands the pending result to the node, then clears it and keeps the stop flag.

        The node runs the ``decorating_result`` hooks of every plugin, sends the result and
        runs the ``after_message_sent`` hooks.
        """
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
        stopped = result.is_stopped()
        chain = wire.chain_to_wire(result.chain)
        if chain:
            event_id = getattr(event, "fish_event_id", None)
            llm = result.result_content_type == ResultContentType.LLM_RESULT
            try:
                await self.runtime.call_node(
                    {
                        "op": "result",
                        "event_id": event_id,
                        "session": event.unified_msg_origin,
                        "chain": chain,
                        "llm": llm,
                    },
                    DELIVER_TIMEOUT_S,
                )
            except TimeoutError:
                log.warning("the node did not confirm the reply within %s s", DELIVER_TIMEOUT_S)
            event._has_send_oper = True
        event.clear_result()
        if stopped:
            event._force_stopped = True

    async def run_llm_request(self, event: AstrMessageEvent, req: ProviderRequest) -> None:
        """Runs a ``ProviderRequest`` a handler yielded through the node's agent, as AstrBot's main agent does.

        The node runs the LLM hooks and tools of every plugin and sends the reply.
        """
        event.set_extra("provider_request", req)
        request: dict[str, Any] = {
            "op": "agent",
            "event_id": getattr(event, "fish_event_id", None),
            "session": event.unified_msg_origin,
            "prompt": adapter.full_prompt(req.prompt, req.extra_user_content_parts),
            "system_prompt": req.system_prompt or "",
            "contexts": adapter._context_dicts(req.contexts) or [],
            "image_urls": list(req.image_urls or []),
        }
        try:
            reply = await self.runtime.call_node(request, AGENT_TIMEOUT_S)
        except TimeoutError:
            log.error("LLM request failed: the node did not answer within %s s", AGENT_TIMEOUT_S)
            return
        if reply.get("error"):
            log.error("LLM request failed: %s", reply["error"])
        if reply.get("stopped"):
            event.stop_event()

    def event_for(self, event_id: Any, data: Any) -> AstrMessageEvent | None:
        """The event of ``event_id``, built from the node's ``data`` when this process has not seen it."""
        if not event_id:
            return None
        event = self.events.get(str(event_id))
        if event is None and isinstance(data, dict):
            event = build_event(data, str(event_id))
            self.remember(str(event_id), event)
        return event

    def plugin_metadata(self, info: dict[str, Any]) -> StarMetadata:
        """The metadata of the plugin a ``plugin_loaded`` or ``plugin_unloaded`` hook is about."""
        name = str(info.get("name") or "")
        for md in star_map.values():
            if md.name == name and (
                self.module_prefix is None or (md.module_path or "").startswith(self.module_prefix)
            ):
                return md
        return StarMetadata(name=name, version=str(info.get("version") or ""), activated=True)

    def agent_run(self, event_id: str, event: AstrMessageEvent, messages: Any) -> ContextWrapper:
        """The agent run context of ``event_id``, created on first use, holding ``messages``."""
        run = self.agent_runs.get(event_id)
        if run is None:
            run = ContextWrapper(context=AstrAgentContext(context=self.instance.context, event=event))
            self.agent_runs[event_id] = run
            while len(self.agent_runs) > self.max_events:
                self.agent_runs.popitem(last=False)
        run.messages = agent_messages(messages)
        return run

    def hook_tool(self, data: Any) -> FunctionTool:
        """The function tool a tool hook is about: this plugin's own tool, or a description of another plugin's."""
        info = data if isinstance(data, dict) else {}
        name = str(info.get("name") or "")
        own = tools.find_tool(name, self.module_prefix)
        if own is not None:
            return own
        parameters = info.get("parameters")
        if not isinstance(parameters, dict) or not parameters:
            parameters = {"type": "object", "properties": {}}
        return FunctionTool(name=name, description=str(info.get("description") or name), parameters=parameters)

    async def handle_hook(self, kind: str, event_id: Any, data: Any, payload: dict[str, Any]) -> dict[str, Any]:
        """Runs this plugin's hooks of one ``kind`` for the node. Returns ``{"payload"?, "stopped"}``.

        ``payload`` carries the values the hooks see. Kinds that let hooks change them
        return the changed values in ``payload``. ``stopped`` is True when a hook of this
        call stopped the event.
        """
        if kind in ("plugin_loaded", "plugin_unloaded"):
            md = self.plugin_metadata(payload.get("plugin") or {})
            etype = EventType.OnPluginLoadedEvent if kind == "plugin_loaded" else EventType.OnPluginUnloadedEvent
            await self.call_hooks(etype, md)
            return {"stopped": False}
        event = self.event_for(event_id, data)
        if event is None:
            return {"stopped": False}
        eid = str(event_id)
        was_stopped = event.is_stopped()
        out: dict[str, Any] = {}
        if kind == "llm_request":
            req = ProviderRequest(
                prompt=payload.get("prompt"),
                system_prompt=payload.get("system_prompt") or "",
                contexts=list(payload.get("contexts") or []),
                image_urls=list(payload.get("image_urls") or []),
            )
            if payload.get("plugin_request"):
                if not isinstance(event.get_extra("provider_request"), ProviderRequest):
                    event.set_extra("provider_request", req)
            else:
                event._extras.pop("provider_request", None)
            await self.call_hooks(EventType.OnWaitingLLMRequestEvent, event)
            if not event.is_stopped():
                await self.call_hooks(EventType.OnLLMRequestEvent, event, req)
            out = {
                "prompt": adapter.full_prompt(req.prompt, req.extra_user_content_parts),
                "system_prompt": req.system_prompt,
                "contexts": adapter._context_dicts(req.contexts) or [],
                "image_urls": list(req.image_urls or []),
            }
        elif kind == "agent_begin":
            await self.call_hooks(
                EventType.OnAgentBeginEvent, event, self.agent_run(eid, event, payload.get("messages"))
            )
        elif kind == "using_llm_tool":
            tool = self.hook_tool(payload.get("tool"))
            await self.call_hooks(EventType.OnUsingLLMToolEvent, event, tool, payload.get("arguments") or {})
        elif kind == "llm_tool_respond":
            tool = self.hook_tool(payload.get("tool"))
            result = CallToolResult(content=[TextContent(type="text", text=str(payload.get("result") or ""))])
            event.clear_result()
            await self.call_hooks(EventType.OnLLMToolRespondEvent, event, tool, payload.get("arguments") or {}, result)
        elif kind == "llm_response":
            response = LLMResponse(
                role="assistant",
                completion_text=str(payload.get("completion_text") or ""),
                reasoning_content=payload.get("reasoning_content") or "",
            )
            await self.call_hooks(EventType.OnLLMResponseEvent, event, response)
            out = {"completion_text": response.completion_text}
        elif kind == "agent_done":
            run = self.agent_run(eid, event, payload.get("messages"))
            self.agent_runs.pop(eid, None)
            response = LLMResponse(role="assistant", completion_text=str(payload.get("completion_text") or ""))
            await self.call_hooks(EventType.OnAgentDoneEvent, event, run, response)
        elif kind in ("decorating_result", "after_message_sent"):
            result = MessageEventResult(chain=wire.chain_from_wire(payload.get("chain")))
            if payload.get("llm"):
                result.set_result_content_type(ResultContentType.LLM_RESULT)
            event.set_result(result)
            if kind == "decorating_result":
                await self.call_hooks(EventType.OnDecoratingResultEvent, event)
                decorated = event.get_result()
                out = {"chain": wire.chain_to_wire(decorated.chain) if decorated is not None else []}
            else:
                await self.call_hooks(EventType.OnAfterMessageSentEvent, event)
            event.clear_result()
        elif kind == "plugin_error":
            error = RuntimeError(str(payload.get("error") or ""))
            await self.call_hooks(
                EventType.OnPluginErrorEvent,
                event,
                str(payload.get("plugin") or ""),
                str(payload.get("handler") or ""),
                error,
                str(payload.get("traceback") or ""),
            )
        reply: dict[str, Any] = {"stopped": event.is_stopped() and not was_stopped}
        if out:
            reply["payload"] = out
        return reply

    async def handle_tool_call(self, event_id: str, data: dict[str, Any], name: str, arguments: Any) -> dict[str, Any]:
        """Runs an ``llm_tool`` handler and returns ``{"text", "sent"}`` or ``{"error"}``.

        Returned and yielded strings form the tool result. Yielded or returned message
        results are sent to the user directly, as in AstrBot.
        """
        tool = tools.find_tool(name, self.module_prefix)
        if tool is None:
            return {"error": f"没有名为 {name} 的工具"}
        event = self.events.get(event_id)
        if event is None:
            event = build_event(data, event_id)
            self.remember(event_id, event)
        args = arguments if isinstance(arguments, dict) else {}
        texts: list[str] = []
        images: list[str] = []
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
                text = tools.result_text(item)
                if text:
                    texts.append(text)
                images.extend(tool_images(item))

        try:
            ret = tools.invoke(tool, event, self.instance.context, args)
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
        out: dict[str, Any] = {"text": text, "sent": sent}
        if images:
            out["images"] = images
        return out
