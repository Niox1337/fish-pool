"""Command filter: matches ``<name> args...`` on woken messages and parses the arguments."""

from __future__ import annotations

import inspect
import re
import types
import typing
from typing import Any

from astrbot.core.star.filter import HandlerFilter


class GreedyStr(str):
    """Annotation for a last parameter that takes all remaining text."""


_EMPTY = inspect.Parameter.empty


def normalize(text: str) -> str:
    return re.sub(r"\s+", " ", text.strip())


def _is_type_spec(value: Any) -> bool:
    return (
        value is _EMPTY
        or isinstance(value, (type, types.UnionType))
        or typing.get_origin(value) is typing.Union
    )


def _convert(raw: str, spec: Any) -> Any:
    """Converts one raw token. ``spec`` is a type annotation or a default value."""
    if spec is _EMPTY or spec is None or spec is str or isinstance(spec, str):
        if spec is None and raw.isdigit():
            return int(raw)
        return raw
    target = spec if _is_type_spec(spec) else type(spec)
    if typing.get_origin(target) in (typing.Union, types.UnionType):
        options = [a for a in typing.get_args(target) if a is not type(None)]
        target = options[0] if len(options) == 1 else str
    if target is bool:
        low = raw.lower()
        if low in ("true", "yes", "1", "on", "开", "是"):
            return True
        if low in ("false", "no", "0", "off", "关", "否"):
            return False
        raise ValueError(raw)
    if target in (int, float, str):
        return target(raw)
    return raw


class CommandFilter(HandlerFilter):
    def __init__(
        self,
        command_name: str,
        alias: set | None = None,
        handler_md: Any = None,
        parent_command_names: list[str] | None = None,
    ) -> None:
        self.command_name = command_name
        self.alias = set(alias or ())
        self.parent_command_names = parent_command_names if parent_command_names is not None else [""]
        self.custom_filter_list: list = []
        self.handler_md = None
        self.handler_params: dict[str, Any] = {}
        if handler_md is not None:
            self.init_handler_md(handler_md)

    def init_handler_md(self, handler_md: Any) -> None:
        self.handler_md = handler_md
        try:
            signature = inspect.signature(handler_md.handler, eval_str=True)
        except Exception:
            signature = inspect.signature(handler_md.handler)
        params = list(signature.parameters.values())[2:]
        self.handler_params = {}
        for p in params:
            if p.kind in (p.VAR_POSITIONAL, p.VAR_KEYWORD):
                continue
            self.handler_params[p.name] = p.annotation if p.default is _EMPTY else p.default

    def get_handler_md(self) -> Any:
        return self.handler_md

    def add_custom_filter(self, custom_filter: Any) -> None:
        self.custom_filter_list.append(custom_filter)

    def custom_filter_ok(self, event: Any, cfg: Any) -> bool:
        return all(f.filter(event, cfg) for f in self.custom_filter_list)

    def get_complete_command_names(self) -> list[str]:
        names = [self.command_name, *sorted(self.alias)]
        return [f"{parent} {name}" if parent else name for name in names for parent in self.parent_command_names or [""]]

    def usage(self) -> str:
        parts = []
        for name, spec in self.handler_params.items():
            if spec is GreedyStr:
                parts.append(f"{name}...")
            elif _is_type_spec(spec):
                parts.append(f"<{name}>")
            else:
                parts.append(f"[{name}={spec}]")
        return " ".join(parts)

    def parse_params(self, tokens: list[str]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        items = list(self.handler_params.items())
        for i, (name, spec) in enumerate(items):
            if spec is GreedyStr:
                if i != len(items) - 1:
                    raise ValueError(f"参数 {name} 是 GreedyStr，必须放在最后")
                result[name] = " ".join(tokens[i:])
                break
            if i >= len(tokens):
                if _is_type_spec(spec):
                    raise ValueError(f"缺少参数 {name}，用法：{self.command_name} {self.usage()}")
                result[name] = spec
                continue
            try:
                result[name] = _convert(tokens[i], spec)
            except (TypeError, ValueError):
                raise ValueError(f"参数 {name} 的值 {tokens[i]} 格式不对，用法：{self.command_name} {self.usage()}")
        return result

    def equals(self, message_str: str) -> bool:
        return normalize(message_str) in self.get_complete_command_names()

    def filter(self, event: Any, cfg: Any) -> bool:
        if not event.is_at_or_wake_command:
            return False
        if not self.custom_filter_ok(event, cfg):
            return False
        text = normalize(event.get_message_str())
        rest = None
        for full in sorted(self.get_complete_command_names(), key=len, reverse=True):
            if text == full or text.startswith(full + " "):
                rest = text[len(full) :].strip()
                break
        if rest is None:
            return False
        tokens = [t for t in rest.split(" ") if t]
        event.set_extra("parsed_params", self.parse_params(tokens))
        return True
