"""AstrBot objects backed by the fish node: message event, chat provider, platform and plugin Context.

Every outgoing message and model call goes through the ``fish_sidecar.runtime``
registered in the plugin process.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import AsyncGenerator, Awaitable
from pathlib import Path
from typing import Any

from astrbot.core.agent.handoff import HandoffTool
from astrbot.core.agent.tool import ToolSet
from astrbot.core.astrbot_config_mgr import AstrBotConfigManager
from astrbot.core.message.components import BaseMessageComponent, Plain
from astrbot.core.message.message_event_result import MessageChain
from astrbot.core.platform.astr_message_event import AstrMessageEvent
from astrbot.core.platform.astrbot_message import AstrBotMessage, MessageMember
from astrbot.core.platform.message_session import MessageSession
from astrbot.core.platform.message_type import MessageType
from astrbot.core.platform.platform import Platform
from astrbot.core.platform.platform_metadata import PlatformMetadata
from astrbot.core.provider.entities import LLMResponse, ProviderMeta, ProviderType, ToolCallsResult
from astrbot.core.provider.provider import EmbeddingProvider, Provider, RerankProvider
from astrbot.core.star.context import Context

from fish_sidecar import runtime, tools, wire
from fish_sidecar.knowledge_base import FishKnowledgeBaseManager

PLATFORM_ADAPTER_NAMES = {"qq_official": "qq_official", "weixin_ilink": "weixin_oc"}
PROVIDER_ID = "fish"


class LLMError(RuntimeError):
    """The node reported a failed model call."""


def _chain_of(message: Any) -> list[BaseMessageComponent]:
    if message is None:
        return []
    if isinstance(message, str):
        return [Plain(message)]
    chain = getattr(message, "chain", message)
    return list(chain or [])


def _context_dicts(contexts: list | None) -> list[dict] | None:
    if not contexts:
        return None
    out = []
    for item in contexts:
        if isinstance(item, dict):
            out.append({"role": "user", "content": "", **item})
        elif hasattr(item, "model_dump"):
            out.append(item.model_dump())
        else:
            out.append({"role": getattr(item, "role", "user"), "content": getattr(item, "content", "")})
    return out


def full_prompt(prompt: str | None, extra_parts: list | None) -> str | None:
    """The prompt followed by the text of ``extra_parts``."""
    extra = []
    for part in extra_parts or []:
        text = part.get("text") if isinstance(part, dict) else getattr(part, "text", None)
        if text:
            extra.append(str(text))
    if not extra:
        return prompt
    return "\n".join([prompt or "", *extra]).strip("\n")


class FishMessageEvent(AstrMessageEvent):
    """Message event whose replies go to the node as ``send`` ops."""

    fish_event_id: str | None = None

    async def send(self, message: MessageChain) -> None:
        chain = _chain_of(message)
        if chain:
            await runtime.get_runtime().send(self.fish_event_id, self.unified_msg_origin, wire.chain_to_wire(chain))
        self._has_send_oper = True

    async def send_streaming(self, generator: AsyncGenerator[MessageChain, None], use_fallback: bool = False) -> None:
        parts: list[BaseMessageComponent] = []
        async for item in generator:
            parts.extend(_chain_of(item))
        await self.send(MessageChain(chain=parts))

    async def send_typing(self) -> None:
        return None

    async def stop_typing(self) -> None:
        return None


def build_event(data: dict[str, Any], event_id: str | None = None) -> FishMessageEvent:
    """Turns a node MessageEvent into an AstrBot message event."""
    is_group = data.get("message_type") == "group"
    woken = bool(data.get("is_at_or_wake"))
    text = str(data.get("message_str") or "")

    msg = AstrBotMessage()
    msg.type = MessageType.GROUP_MESSAGE if is_group else MessageType.FRIEND_MESSAGE
    msg.self_id = str(data.get("self_id") or "")
    msg.session_id = str(data.get("session_id") or "")
    msg.message_id = str(data.get("message_id") or "")
    if is_group:
        msg.group_id = str(data.get("group_id") or "")
    sender = data.get("sender") or {}
    msg.sender = MessageMember(user_id=str(sender.get("user_id") or ""), nickname=sender.get("nickname"))
    msg.message = wire.chain_from_wire(data.get("chain"))
    msg.message_str = text
    msg.raw_message = data.get("raw")
    ts = data.get("timestamp") or time.time()
    msg.timestamp = int(ts / 1000) if ts > 1e11 else int(ts)

    platform = str(data.get("platform") or "")
    umo = str(data.get("unified_msg_origin") or "")
    try:
        session = MessageSession.from_str(umo)
    except Exception:
        session = MessageSession(platform, msg.type, msg.session_id)
    meta = PlatformMetadata(
        name=PLATFORM_ADAPTER_NAMES.get(platform, platform), description="fish", id=session.platform_name
    )
    event = FishMessageEvent(text, msg, meta, session.session_id)
    event.session = session
    event.is_at_or_wake_command = woken
    event.is_wake = woken
    event.role = "admin" if data.get("is_admin") else "member"
    event.fish_event_id = event_id
    return event


class FishPlatform(Platform):
    """Platform entry for one platform id seen in events. Sends through the node."""

    def __init__(self, metadata: PlatformMetadata) -> None:
        super().__init__({"id": metadata.id, "type": metadata.name}, asyncio.Queue())
        self._metadata = metadata

    def meta(self) -> PlatformMetadata:
        return self._metadata

    async def run(self) -> None:
        return None

    async def send_by_session(self, session: MessageSession, message_chain: MessageChain) -> None:
        chain = _chain_of(message_chain)
        if chain:
            await runtime.get_runtime().send_to(str(session), wire.chain_to_wire(chain))


class FishPlatformManager:
    def __init__(self) -> None:
        self.platform_insts: list[Platform] = []

    def remember(self, metadata: PlatformMetadata) -> None:
        if not any(p.meta().id == metadata.id for p in self.platform_insts):
            self.platform_insts.append(FishPlatform(metadata))

    def get_insts(self) -> list[Platform]:
        return self.platform_insts


class FishProvider(Provider):
    """Chat provider that asks the node to run the bot's configured model."""

    def __init__(self) -> None:
        super().__init__({"id": PROVIDER_ID, "type": PROVIDER_ID, "model": PROVIDER_ID}, {})
        self.model_name = PROVIDER_ID

    def meta(self) -> ProviderMeta:
        return ProviderMeta(
            id=PROVIDER_ID, model=self.model_name, type=PROVIDER_ID, provider_type=ProviderType.CHAT_COMPLETION
        )

    def get_current_key(self) -> str:
        return ""

    def get_keys(self) -> list[str]:
        return []

    def set_key(self, key: str) -> None:
        return None

    async def get_models(self) -> list[str]:
        return [self.model_name]

    async def text_chat(
        self,
        prompt: str | None = None,
        session_id: str | None = None,
        image_urls: list[str] | None = None,
        audio_urls: list[str] | None = None,
        func_tool: ToolSet | None = None,
        contexts: list | None = None,
        system_prompt: str | None = None,
        tool_calls_result: ToolCallsResult | list[ToolCallsResult] | None = None,
        model: str | None = None,
        extra_user_content_parts: list | None = None,
        **kwargs: Any,
    ) -> LLMResponse:
        if prompt is not None or not contexts:
            prompt = full_prompt(prompt, extra_user_content_parts)
        messages = _context_dicts(contexts) or []
        images = [u for u in image_urls or [] if isinstance(u, str)]
        if tool_calls_result:
            if prompt is not None or images:
                parts = [{"type": "text", "text": prompt or ""}]
                parts += [{"type": "image_url", "image_url": {"url": url}} for url in images]
                messages.append({"role": "user", "content": parts})
                prompt, images = None, []
            results = tool_calls_result if isinstance(tool_calls_result, list) else [tool_calls_result]
            for result in results:
                messages.extend(result.to_openai_messages())
        definitions = [
            tools.tool_schema(tool)
            for tool in (func_tool.tools if func_tool else [])
            if tool.active and not isinstance(tool, HandoffTool)
        ]
        reply = await runtime.get_runtime().llm(
            prompt,
            system_prompt or None,
            messages,
            images,
            definitions,
        )
        calls = (reply.get("tool_calls") or []) if definitions else []
        if any(not isinstance(call["arguments"], dict) for call in calls):
            raise LLMError("模型返回的工具参数不是 JSON 对象")
        return LLMResponse(
            role="tool" if calls else "assistant",
            completion_text=reply.get("text") or "",
            tools_call_ids=[call["id"] for call in calls],
            tools_call_name=[call["name"] for call in calls],
            tools_call_args=[call["arguments"] for call in calls],
            tools_call_extra_content=reply.get("tools_call_extra_content"),
            reasoning_content=reply.get("reasoning_content"),
            reasoning_signature=reply.get("reasoning_signature"),
        )

    async def text_chat_stream(self, *args: Any, **kwargs: Any) -> AsyncGenerator[LLMResponse, None]:
        yield await self.text_chat(*args, **kwargs)


class FishProviderManager:
    """Provider registry with the fish chat provider as the only chat model."""

    def __init__(self, llm_tools: Any, config: AstrBotConfigManager, persona_mgr: Any = None) -> None:
        self.persona_mgr = persona_mgr
        self.default_persona_name = getattr(persona_mgr, "default_persona", "default")
        self.provider = FishProvider()
        self.provider_insts: list[Provider] = [self.provider]
        self.tts_provider_insts: list = []
        self.stt_provider_insts: list = []
        self.embedding_provider_insts: list = []
        self.rerank_provider_insts: list = []
        self.decision_provider_insts: list = []
        self.inst_map: dict[str, Any] = {PROVIDER_ID: self.provider}
        self.llm_tools = llm_tools
        self.acm = config
        self.curr_provider_inst = self.provider

    @property
    def persona_configs(self) -> list:
        return getattr(self.persona_mgr, "persona_v3_config", [])

    @property
    def personas(self) -> list:
        return getattr(self.persona_mgr, "personas_v3", [])

    @property
    def selected_default_persona(self) -> Any:
        return getattr(self.persona_mgr, "selected_default_persona_v3", None)

    def get_using_provider(
        self, provider_type: ProviderType = ProviderType.CHAT_COMPLETION, umo: str | None = None
    ) -> Any:
        return self.provider if provider_type == ProviderType.CHAT_COMPLETION else None

    async def get_using_provider_async(
        self, provider_type: ProviderType = ProviderType.CHAT_COMPLETION, umo: str | None = None
    ) -> Any:
        return self.get_using_provider(provider_type, umo)

    async def get_provider_by_id(self, provider_id: str) -> Any:
        return self.inst_map.get(provider_id)

    def get_merged_provider_config(self, provider_config: dict) -> dict:
        return dict(provider_config)


class FishContext(Context):
    """Plugin Context whose messaging, model and task methods go through the node."""

    def get_config(self, umo: str | None = None) -> Any:
        return self.astrbot_config_mgr.get_conf(umo)

    def get_provider_by_id(self, provider_id: str) -> Any:
        return self.provider_manager.inst_map.get(provider_id) or self.provider_manager.provider

    def register_provider(self, provider: Provider | EmbeddingProvider | RerankProvider) -> None:
        manager = self.provider_manager
        provider_id = provider.provider_config["id"]
        if provider_id in manager.inst_map:
            raise ValueError(f"Provider {provider_id} is already registered")
        if isinstance(provider, EmbeddingProvider):
            manager.embedding_provider_insts.append(provider)
        elif isinstance(provider, RerankProvider):
            manager.rerank_provider_insts.append(provider)
        else:
            super().register_provider(provider)
        manager.inst_map[provider_id] = provider

    async def send_message(self, session: Any, message_chain: MessageChain) -> bool:
        chain = _chain_of(message_chain)
        if chain:
            await runtime.get_runtime().send_to(str(session), wire.chain_to_wire(chain))
        return True

    def register_task(self, task: Awaitable, desc: str) -> None:
        runtime.get_runtime().start_task(task, desc)


async def build_context() -> FishContext:
    """Creates the plugin Context on the current AstrBot root. Call after ``astrbot_boot.enter_plugin_root``."""
    import astrbot.core as core
    from astrbot.core import astrbot_config_mgr
    from astrbot.core.conversation_mgr import ConversationManager
    from astrbot.core.cron.manager import CronJobManager
    from astrbot.core.knowledge_base import kb_mgr
    from astrbot.core.persona_mgr import PersonaManager
    from astrbot.core.platform_message_history_mgr import PlatformMessageHistoryManager
    from astrbot.core.provider.register import llm_tools
    from astrbot.core.umop_config_router import UmopConfigRouter
    from astrbot.core.utils.astrbot_path import get_astrbot_knowledge_base_path

    # A group process keeps one database per plugin on the runtime. A plugin process uses the rebound globals.
    current = runtime.current()
    db = getattr(current, "db", None) or core.db_helper
    sp = getattr(current, "sp", None) or core.sp
    router = UmopConfigRouter(sp)
    config = astrbot_config_mgr.AstrBotConfigManager(core.astrbot_config, router, sp)
    await router.initialize()
    await config.initialize()
    persona = PersonaManager(db, config)
    await persona.initialize()
    provider_manager = FishProviderManager(llm_tools, config, persona)

    # These paths were captured when AstrBot was imported in the fork server.
    astrbot_config_mgr.DEFAULT_CONFIG_CONF_INFO["path"] = core.astrbot_config.config_path
    kb_mgr.FILES_PATH = get_astrbot_knowledge_base_path()
    kb_mgr.DB_PATH = Path(kb_mgr.FILES_PATH) / "kb.db"
    cron = CronJobManager(db)
    context = FishContext(
        asyncio.Queue(),
        core.astrbot_config,
        db,
        provider_manager,  # type: ignore[arg-type]
        FishPlatformManager(),
        ConversationManager(db),
        PlatformMessageHistoryManager(db),
        persona,
        config,
        FishKnowledgeBaseManager(provider_manager),  # type: ignore[arg-type]
        cron,
        None,
    )
    cron.ctx = context
    return context
