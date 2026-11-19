from __future__ import annotations

import enum
from typing import Any

from astrbot.core.platform.message_type import MessageType
from astrbot.core.star.filter import HandlerFilter


class EventMessageType(enum.Flag):
    GROUP_MESSAGE = enum.auto()
    PRIVATE_MESSAGE = enum.auto()
    OTHER_MESSAGE = enum.auto()
    ALL = GROUP_MESSAGE | PRIVATE_MESSAGE | OTHER_MESSAGE


_FROM_MESSAGE_TYPE = {
    MessageType.GROUP_MESSAGE: EventMessageType.GROUP_MESSAGE,
    MessageType.FRIEND_MESSAGE: EventMessageType.PRIVATE_MESSAGE,
    MessageType.OTHER_MESSAGE: EventMessageType.OTHER_MESSAGE,
}


class EventMessageTypeFilter(HandlerFilter):
    def __init__(self, event_message_type: EventMessageType) -> None:
        self.event_message_type = event_message_type

    def filter(self, event: Any, cfg: Any) -> bool:
        flag = _FROM_MESSAGE_TYPE.get(event.get_message_type())
        return bool(flag and flag & self.event_message_type)
