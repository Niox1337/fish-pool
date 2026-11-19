from __future__ import annotations

from dataclasses import dataclass, field

from astrbot.core.platform.message_type import MessageType


@dataclass
class MessageSession:
    """Identifies one conversation. Its string form is the unified message origin."""

    platform_name: str
    message_type: MessageType
    session_id: str
    platform_id: str = field(init=False)

    def __post_init__(self) -> None:
        self.platform_id = self.platform_name

    def __str__(self) -> str:
        return f"{self.platform_id}:{self.message_type.value}:{self.session_id}"

    @staticmethod
    def from_str(session_str: str) -> MessageSession:
        platform_id, message_type, session_id = session_str.split(":", 2)
        return MessageSession(platform_id, MessageType(message_type), session_id)


MessageSesion = MessageSession
