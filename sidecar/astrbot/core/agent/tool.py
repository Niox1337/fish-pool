"""Function tool declarations. They are recorded but never offered to the model."""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any


@dataclass
class FunctionTool:
    name: str = ""
    description: str = ""
    parameters: dict = field(default_factory=dict)
    handler: Callable[..., Awaitable[Any]] | None = None
    handler_module_path: str | None = None
    active: bool = True

    async def call(self, context: Any, **kwargs: Any) -> Any:
        if self.handler is None:
            raise NotImplementedError(f"FunctionTool {self.name} has no handler")
        return await self.handler(context, **kwargs)


class ToolSet:
    def __init__(self, tools: list[FunctionTool] | None = None) -> None:
        self.tools: list[FunctionTool] = list(tools or [])

    def add_tool(self, tool: FunctionTool) -> None:
        self.tools.append(tool)

    def remove_tool(self, name: str) -> None:
        self.tools = [t for t in self.tools if t.name != name]

    def get_tool(self, name: str) -> FunctionTool | None:
        return next((t for t in self.tools if t.name == name), None)

    def empty(self) -> bool:
        return not self.tools

    def __len__(self) -> int:
        return len(self.tools)


ToolExecResult = Any
