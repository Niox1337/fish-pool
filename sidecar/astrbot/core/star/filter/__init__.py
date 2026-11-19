from __future__ import annotations

import abc
from typing import Any


class HandlerFilter(abc.ABC):
    """Decides whether a handler should run for an event."""

    @abc.abstractmethod
    def filter(self, event: Any, cfg: Any) -> bool:
        raise NotImplementedError


__all__ = ["HandlerFilter"]
