from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any

from astrbot.core.message.components import BaseMessageComponent
from astrbot.core.platform.message_type import MessageType


@dataclass
class MessageMember:
    user_id: str
    nickname: str | None = None


@dataclass
class Group:
    group_id: str
    group_name: str | None = None
    group_avatar: str | None = None
    group_owner: str | None = None
    group_admins: list[str] | None = None
    members: list[MessageMember] | None = None
    member_count: int | None = None


class AstrBotMessage:
    """Platform independent description of one inbound message."""

    type: MessageType
    self_id: str
    session_id: str
    message_id: str
    group: Group | None
    sender: MessageMember
    message: list[BaseMessageComponent]
    message_str: str
    raw_message: Any
    timestamp: int

    def __init__(self) -> None:
        self.type = MessageType.OTHER_MESSAGE
        self.self_id = ""
        self.session_id = ""
        self.message_id = ""
        self.group = None
        self.sender = MessageMember(user_id="")
        self.message = []
        self.message_str = ""
        self.raw_message = None
        self.timestamp = int(time.time())

    @property
    def group_id(self) -> str:
        return self.group.group_id if self.group else ""

    @group_id.setter
    def group_id(self, value: str | None) -> None:
        if value:
            if self.group:
                self.group.group_id = value
            else:
                self.group = Group(group_id=value)
        else:
            self.group = None

    def __str__(self) -> str:
        return str(self.__dict__)
