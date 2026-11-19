from __future__ import annotations

import re
from typing import Any

from astrbot.core.star.filter import HandlerFilter


class RegexFilter(HandlerFilter):
    """Searches the stripped message text. Works without a wake prefix."""

    def __init__(self, regex: str | re.Pattern) -> None:
        self.regex = re.compile(regex)
        self.regex_str = self.regex.pattern

    def filter(self, event: Any, cfg: Any) -> bool:
        return bool(self.regex.search(event.get_message_str().strip()))
