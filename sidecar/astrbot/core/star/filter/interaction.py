from __future__ import annotations

from typing import Any

from astrbot.core.star.filter import HandlerFilter


class ButtonActionFilter(HandlerFilter):
    """Never matches. Fish platforms do not deliver button interactions."""

    def __init__(self, action_id: str | None = None) -> None:
        self.action_id = action_id

    def filter(self, event: Any, cfg: Any) -> bool:
        return False


class UIActionFilter(ButtonActionFilter):
    """Never matches. Fish platforms do not deliver plugin view actions."""
