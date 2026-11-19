"""Command group filter. Sub commands match through their own CommandFilter."""

from __future__ import annotations

from typing import Any

from astrbot.core.star.filter import HandlerFilter
from astrbot.core.star.filter.command import CommandFilter, normalize


class CommandGroupFilter(HandlerFilter):
    def __init__(self, group_name: str, alias: set | None = None, parent_group: CommandGroupFilter | None = None) -> None:
        self.group_name = group_name
        self.alias = set(alias or ())
        self.parent_group = parent_group
        self.sub_command_filters: list[CommandFilter | CommandGroupFilter] = []
        self.custom_filter_list: list = []

    def add_sub_command_filter(self, sub: CommandFilter | CommandGroupFilter) -> None:
        self.sub_command_filters.append(sub)

    def add_custom_filter(self, custom_filter: Any) -> None:
        self.custom_filter_list.append(custom_filter)

    def custom_filter_ok(self, event: Any, cfg: Any) -> bool:
        return all(f.filter(event, cfg) for f in self.custom_filter_list)

    def get_complete_command_names(self) -> list[str]:
        own = [self.group_name, *sorted(self.alias)]
        if self.parent_group is None:
            return own
        return [f"{parent} {name}" for parent in self.parent_group.get_complete_command_names() for name in own]

    def help_text(self, indent: str = "") -> str:
        lines = [] if indent else [f"{self.group_name} 的子指令："]
        for sub in self.sub_command_filters:
            if isinstance(sub, CommandGroupFilter):
                lines.append(f"{indent}  {sub.group_name}")
                lines.append(sub.help_text(indent + "  "))
            else:
                desc = getattr(sub.handler_md, "desc", "") if sub.handler_md else ""
                usage = sub.usage()
                line = f"{indent}  {sub.command_name}"
                if usage:
                    line += f" {usage}"
                if desc:
                    line += f"  {desc.splitlines()[0]}"
                lines.append(line)
        return "\n".join(line for line in lines if line)

    def startswith(self, message_str: str) -> bool:
        text = normalize(message_str)
        return any(text == n or text.startswith(n + " ") for n in self.get_complete_command_names())

    def equals(self, message_str: str) -> bool:
        return normalize(message_str) in self.get_complete_command_names()

    def filter(self, event: Any, cfg: Any) -> bool:
        if not event.is_at_or_wake_command or not self.custom_filter_ok(event, cfg):
            return False
        if self.equals(event.get_message_str()):
            raise ValueError(self.help_text())
        return self.startswith(event.get_message_str())
