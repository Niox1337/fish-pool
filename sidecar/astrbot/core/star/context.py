"""The plugin context: LLM access, proactive sending, configuration and background tasks."""

from __future__ import annotations

import logging
from collections.abc import Awaitable
from typing import Any

from astrbot import _fish, _wire
from astrbot._unsupported import UnsupportedObject, unsupported
from astrbot.core.config.astrbot_config import AstrBotConfig
from astrbot.core.conversation_mgr import ConversationManager
from astrbot.core.message.message_event_result import MessageChain
from astrbot.core.platform.message_session import MessageSession
from astrbot.core.provider.entities import LLMResponse
from astrbot.core.provider.provider import FishProvider, Provider
from astrbot.core.star.star import StarMetadata, star_registry

log = logging.getLogger("astrbot")

DEFAULT_CORE_CONFIG = {
    "wake_prefix": ["/"],
    "admins_id": [],
    "provider_settings": {"enable": True, "wake_prefix": "", "web_search": False},
    "platform_settings": {"unique_session": False, "reply_prefix": "", "forward_threshold": 1500},
    "t2i": False,
    "plugin_set": ["*"],
    "disabled_plugins": [],
    "timezone": "Asia/Shanghai",
    "default_personality": "default",
}


class Context:
    def __init__(self, config: AstrBotConfig | None = None, **_: Any) -> None:
        self._config = config if config is not None else AstrBotConfig(default_config=DEFAULT_CORE_CONFIG)
        self._provider = FishProvider()
        self._register_tasks: list[Awaitable] = []
        self.conversation_manager = ConversationManager()
        self.html_renderer = UnsupportedObject("Context.html_renderer")
        self.persona_manager = UnsupportedObject("Context.persona_manager")
        self.provider_manager = UnsupportedObject("Context.provider_manager")
        self.platform_manager = UnsupportedObject("Context.platform_manager")
        self.message_history_manager = UnsupportedObject("Context.message_history_manager")
        self.kb_manager = UnsupportedObject("Context.kb_manager")
        self.cron_manager = UnsupportedObject("Context.cron_manager")
        self.astrbot_config_mgr = UnsupportedObject("Context.astrbot_config_mgr")

    def get_config(self, umo: str | None = None) -> AstrBotConfig:
        return self._config

    def locale(self, umo: str | None = None) -> str:
        return "zh-CN"

    def get_using_provider(self, umo: str | None = None) -> Provider:
        return self._provider

    async def get_using_provider_async(self, umo: str | None = None) -> Provider:
        return self._provider

    async def get_current_chat_provider_id(self, umo: str) -> str:
        return "fish"

    def get_provider_by_id(self, provider_id: str) -> Provider:
        return self._provider

    def get_all_providers(self) -> list[Provider]:
        return [self._provider]

    def get_using_tts_provider(self, umo: str | None = None) -> None:
        return None

    async def get_using_tts_provider_async(self, umo: str | None = None) -> None:
        return None

    def get_using_stt_provider(self, umo: str | None = None) -> None:
        return None

    async def get_using_stt_provider_async(self, umo: str | None = None) -> None:
        return None

    def get_all_tts_providers(self) -> list:
        return []

    def get_all_stt_providers(self) -> list:
        return []

    def get_all_embedding_providers(self) -> list:
        return []

    async def llm_generate(
        self,
        *,
        chat_provider_id: str | None = None,
        prompt: str | None = None,
        image_urls: list[str] | None = None,
        audio_urls: list[str] | None = None,
        tools: Any = None,
        system_prompt: str | None = None,
        contexts: list | None = None,
        **kwargs: Any,
    ) -> LLMResponse:
        if tools:
            log.warning("llm_generate ignores tools, function calling is not available")
        return await self._provider.text_chat(prompt=prompt, image_urls=image_urls, contexts=contexts, system_prompt=system_prompt)

    async def tool_loop_agent(self, *args: Any, **kwargs: Any) -> Any:
        unsupported("Context.tool_loop_agent")

    async def send_message(self, session: str | MessageSession, message_chain: MessageChain | list) -> bool:
        """Sends a proactive message to a unified message origin."""
        if isinstance(session, str):
            try:
                MessageSession.from_str(session)
            except Exception as e:
                raise ValueError(f"invalid session string: {e}") from e
        chain = _wire.chain_to_wire(message_chain)
        if not chain:
            return True
        await _fish.get_runtime().send_to(str(session), chain)
        return True

    def get_registered_star(self, star_name: str) -> StarMetadata | None:
        return next((md for md in star_registry if md.name == star_name), None)

    def get_all_stars(self) -> list[StarMetadata]:
        return list(star_registry)

    def register_task(self, task: Awaitable, desc: str) -> None:
        self._register_tasks.append(task)
        if _fish.has_runtime():
            _fish.get_runtime().start_task(task, desc)

    def get_llm_tool_manager(self) -> Any:
        unsupported("Context.get_llm_tool_manager")

    def activate_llm_tool(self, name: str) -> bool:
        return False

    async def activate_llm_tool_async(self, name: str) -> bool:
        return False

    def deactivate_llm_tool(self, name: str) -> bool:
        return False

    async def deactivate_llm_tool_async(self, name: str) -> bool:
        return False

    def add_llm_tools(self, *tools: Any) -> None:
        log.warning("function tools are recorded but never offered to the model")

    def register_llm_tool(self, name: str, func_args: list, desc: str, func_obj: Any) -> None:
        log.warning("function tool %s is recorded but never offered to the model", name)

    def unregister_llm_tool(self, name: str) -> None:
        return None

    def register_web_api(self, *args: Any, **kwargs: Any) -> None:
        unsupported("Context.register_web_api")

    def register_commands(self, *args: Any, **kwargs: Any) -> None:
        unsupported("Context.register_commands")

    def register_provider(self, provider: Any) -> None:
        unsupported("Context.register_provider")

    def get_event_queue(self) -> Any:
        unsupported("Context.get_event_queue")

    def get_platform(self, platform_type: Any) -> Any:
        unsupported("Context.get_platform")

    def get_platform_inst(self, platform_id: str) -> Any:
        unsupported("Context.get_platform_inst")

    def get_db(self) -> Any:
        unsupported("Context.get_db")
