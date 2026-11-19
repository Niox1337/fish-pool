from __future__ import annotations

from abc import ABCMeta, abstractmethod
from typing import Any

from astrbot.core.star.filter import HandlerFilter


class CustomFilterMeta(ABCMeta):
    def __and__(cls, other):
        return CustomFilterAnd(cls(), other())

    def __or__(cls, other):
        return CustomFilterOr(cls(), other())


class CustomFilter(HandlerFilter, metaclass=CustomFilterMeta):
    """Base class for plugin defined filters. Subclasses combine with ``&`` and ``|``."""

    def __init__(self, raise_error: bool = True, **kwargs: Any) -> None:
        self.raise_error = raise_error

    @abstractmethod
    def filter(self, event: Any, cfg: Any) -> bool:
        raise NotImplementedError

    def __and__(self, other: CustomFilter) -> CustomFilterAnd:
        return CustomFilterAnd(self, other)

    def __or__(self, other: CustomFilter) -> CustomFilterOr:
        return CustomFilterOr(self, other)


class CustomFilterAnd(CustomFilter):
    def __init__(self, first: CustomFilter, second: CustomFilter) -> None:
        super().__init__()
        self.first, self.second = first, second

    def filter(self, event: Any, cfg: Any) -> bool:
        return self.first.filter(event, cfg) and self.second.filter(event, cfg)


class CustomFilterOr(CustomFilter):
    def __init__(self, first: CustomFilter, second: CustomFilter) -> None:
        super().__init__()
        self.first, self.second = first, second

    def filter(self, event: Any, cfg: Any) -> bool:
        return self.first.filter(event, cfg) or self.second.filter(event, cfg)
