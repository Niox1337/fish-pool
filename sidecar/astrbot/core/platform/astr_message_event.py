"""The message event object that handlers receive."""

from __future__ import annotations

import os
import re
import time
from collections.abc import AsyncGenerator
from typing import Any

from astrbot import _fish, _wire, logger
from astrbot._unsupported import unsupported
from astrbot.core.message.components import BaseMessageComponent, Plain
from astrbot.core.message.message_event_result import MessageChain, MessageEventResult
from astrbot.core.platform.astrbot_message import AstrBotMessage, Group
from astrbot.core.platform.message_session import MessageSession
from astrbot.core.platform.message_type import MessageType
from astrbot.core.platform.platform_metadata import PlatformMetadata
from astrbot.core.provider.entities import ProviderRequest


class AstrMessageEvent:
    def __init__(
        self,
        message_str: str,
        message_obj: AstrBotMessage,
        platform_meta: PlatformMetadata,
        session_id: str,
    ) -> None:
        self.message_str = message_str
        self.message_obj = message_obj
        self.platform_meta = platform_meta
        self.platform = platform_meta
        self.role = "member"
        self.is_wake = False
        self.is_at_or_wake_command = False
        self.call_llm = False
        self.plugins_name: list[str] | None = None
        self.created_at = time.time()
        self.session = MessageSession(platform_meta.id or platform_meta.name, message_obj.type, session_id)
        self._extras: dict[str, Any] = {}
        self._force_stopped = False
        self._result: MessageEventResult | None = None
        self._has_send_oper = False
        self._temporary_local_files: list[str] = []
        self._fish_event_id: str | None = None

    @property
    def unified_msg_origin(self) -> str:
        return str(self.session)

    @unified_msg_origin.setter
    def unified_msg_origin(self, value: str) -> None:
        self.session = MessageSession.from_str(value)

    @property
    def session_id(self) -> str:
        return self.session.session_id

    @session_id.setter
    def session_id(self, value: str) -> None:
        self.session.session_id = value

    def get_platform_name(self) -> str:
        return self.platform_meta.name

    def get_platform_id(self) -> str:
        return self.platform_meta.id

    def get_message_str(self) -> str:
        return self.message_str

    def get_message_outline(self) -> str:
        parts = []
        for comp in self.message_obj.message or []:
            parts.append(comp.text if isinstance(comp, Plain) else f"[{type(comp).__name__}]")
        return "".join(parts)

    def get_messages(self) -> list[BaseMessageComponent]:
        return self.message_obj.message

    def get_message_type(self) -> MessageType:
        message_type = getattr(self.message_obj, "type", None)
        if isinstance(message_type, MessageType):
            return message_type
        return self.session.message_type

    def get_session_id(self) -> str:
        return self.session_id

    def get_group_id(self) -> str:
        return getattr(self.message_obj, "group_id", "") or ""

    def get_self_id(self) -> str:
        return getattr(self.message_obj, "self_id", "") or ""

    def get_sender_id(self) -> str:
        sender = getattr(self.message_obj, "sender", None)
        user_id = getattr(sender, "user_id", None)
        return str(user_id) if user_id is not None else ""

    def get_sender_name(self) -> str:
        sender = getattr(self.message_obj, "sender", None)
        nickname = getattr(sender, "nickname", None)
        return "" if nickname is None else str(nickname)

    def set_extra(self, key: str, value: Any) -> None:
        self._extras[key] = value

    def get_extra(self, key: str | None = None, default: Any = None) -> Any:
        if key is None:
            return self._extras
        return self._extras.get(key, default)

    def clear_extra(self) -> None:
        self._extras.clear()

    def track_temporary_local_file(self, path: str) -> None:
        if path and path not in self._temporary_local_files:
            self._temporary_local_files.append(path)

    def untrack_temporary_local_file(self, path: str) -> None:
        if path in self._temporary_local_files:
            self._temporary_local_files.remove(path)

    def cleanup_temporary_local_files(self) -> None:
        paths, self._temporary_local_files = self._temporary_local_files, []
        for path in paths:
            try:
                if os.path.exists(path):
                    os.remove(path)
            except OSError as e:
                logger.warning("failed to remove temporary file %s: %s", path, e)

    def is_private_chat(self) -> bool:
        return self.get_message_type() == MessageType.FRIEND_MESSAGE

    def is_wake_up(self) -> bool:
        return self.is_wake

    def is_admin(self) -> bool:
        return self.role == "admin"

    def is_button_interaction(self) -> bool:
        return False

    def get_button_interaction(self) -> None:
        return None

    def get_ui_interaction(self) -> None:
        return None

    def view_result(self, *args: Any, **kwargs: Any) -> Any:
        unsupported("AstrMessageEvent.view_result")

    def set_result(self, result: MessageEventResult | str) -> None:
        if isinstance(result, str):
            result = MessageEventResult().message(result)
        if isinstance(result, MessageEventResult) and result.chain is None:
            result.chain = []
        self._result = result

    def stop_event(self) -> None:
        self._force_stopped = True
        if self._result is None:
            self.set_result(MessageEventResult().stop_event())
        else:
            self._result.stop_event()

    def continue_event(self) -> None:
        self._force_stopped = False
        if self._result is None:
            self.set_result(MessageEventResult().continue_event())
        else:
            self._result.continue_event()

    def is_stopped(self) -> bool:
        if self._force_stopped:
            return True
        return self._result is not None and self._result.is_stopped()

    def should_call_llm(self, call_llm: bool) -> None:
        """Passing True blocks the default LLM reply for this event."""
        self.call_llm = call_llm

    def get_result(self) -> MessageEventResult | None:
        return self._result

    def clear_result(self) -> None:
        self._result = None

    def make_result(self) -> MessageEventResult:
        return MessageEventResult()

    def plain_result(self, text: str) -> MessageEventResult:
        return MessageEventResult().message(text)

    def image_result(self, url_or_path: str) -> MessageEventResult:
        if url_or_path.startswith("http"):
            return MessageEventResult().url_image(url_or_path)
        return MessageEventResult().file_image(url_or_path)

    def chain_result(self, chain: list[BaseMessageComponent]) -> MessageEventResult:
        result = MessageEventResult()
        result.chain = list(chain)
        return result

    def request_llm(
        self,
        prompt: str,
        func_tool_manager: Any = None,
        tool_set: Any = None,
        session_id: str = "",
        image_urls: list[str] | None = None,
        audio_urls: list[str] | None = None,
        contexts: list | None = None,
        system_prompt: str = "",
        conversation: Any = None,
    ) -> ProviderRequest:
        """Builds a request. Yield it from a handler and the pipeline sends it to the LLM."""
        contexts = list(contexts or [])
        return ProviderRequest(
            prompt=prompt,
            session_id=session_id,
            image_urls=list(image_urls or []),
            audio_urls=list(audio_urls or []),
            func_tool=tool_set,
            contexts=contexts,
            system_prompt=system_prompt,
            conversation=None if contexts else conversation,
        )

    async def send(self, message: MessageChain | list | str) -> None:
        chain = _wire.chain_to_wire(message)
        self._has_send_oper = True
        if chain:
            await _fish.get_runtime().send(self._fish_event_id, self.unified_msg_origin, chain)

    async def send_streaming(self, generator: AsyncGenerator, use_fallback: bool = False) -> None:
        parts: list[BaseMessageComponent] = []
        async for item in generator:
            if isinstance(item, MessageChain):
                parts.extend(item.chain)
        squashed = MessageChain(chain=parts).squash_plain()
        if squashed is not None:
            await self.send(squashed)

    async def process_buffer(self, buffer: str, pattern: re.Pattern) -> str:
        while match := re.search(pattern, buffer):
            text = buffer[: match.end()]
            if text.strip():
                await self.send(MessageChain([Plain(text)]))
            buffer = buffer[match.end() :]
        return buffer

    async def send_typing(self) -> None:
        return None

    async def stop_typing(self) -> None:
        return None

    async def react(self, emoji: str) -> None:
        await self.send(MessageChain([Plain(emoji)]))

    async def get_group(self, group_id: str | None = None, **kwargs: Any) -> Group | None:
        resolved = group_id or self.get_group_id()
        if not resolved:
            return None
        if self.message_obj.group and self.message_obj.group.group_id == str(resolved):
            return self.message_obj.group
        return Group(group_id=str(resolved))

    async def _pre_send(self) -> None:
        return None

    async def _post_send(self) -> None:
        return None
