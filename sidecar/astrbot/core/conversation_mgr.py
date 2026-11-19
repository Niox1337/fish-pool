"""Conversation manager facade. Conversation history lives in the fish node, not in plugins."""

from __future__ import annotations

from typing import Any

from astrbot._unsupported import unsupported


class ConversationManager:
    """Reports that no plugin visible conversation exists for any session.

    shortcut: conversations are not exposed to plugins, add node ops for reading and
    writing conversations when plugins that manage history need to run.
    """

    async def get_curr_conversation_id(self, unified_msg_origin: str) -> str | None:
        return None

    async def get_conversation(self, unified_msg_origin: str, conversation_id: str, create_if_not_exists: bool = False) -> Any:
        return None

    async def get_conversations(self, unified_msg_origin: str | None = None, platform_id: str | None = None) -> list:
        return []

    async def new_conversation(self, *args: Any, **kwargs: Any) -> str:
        unsupported("ConversationManager.new_conversation")

    async def switch_conversation(self, *args: Any, **kwargs: Any) -> None:
        unsupported("ConversationManager.switch_conversation")

    async def delete_conversation(self, *args: Any, **kwargs: Any) -> None:
        unsupported("ConversationManager.delete_conversation")

    async def update_conversation(self, *args: Any, **kwargs: Any) -> None:
        unsupported("ConversationManager.update_conversation")

    async def add_message_pair(self, *args: Any, **kwargs: Any) -> None:
        unsupported("ConversationManager.add_message_pair")
