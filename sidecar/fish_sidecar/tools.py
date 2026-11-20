"""Function tools registered in AstrBot's tool manager, as seen by the node.

A plugin process runs a single plugin, so every active function tool in
``llm_tools`` belongs to it. Sub agent handoff tools are not exposed.
"""

from __future__ import annotations

import inspect
import re
from typing import Any

from astrbot.core.agent.handoff import FunctionTool, HandoffTool
from astrbot.core.provider.register import llm_tools

TOOL_NAME = re.compile(r"^[A-Za-z0-9_-]{1,64}$")


def plugin_tools() -> list[FunctionTool]:
    """Active function tools, the last registration of each name winning."""
    out: dict[str, FunctionTool] = {}
    for tool in llm_tools.func_list:
        if isinstance(tool, HandoffTool) or not getattr(tool, "active", True):
            continue
        if isinstance(tool.name, str) and TOOL_NAME.match(tool.name):
            out[tool.name] = tool
    return list(out.values())


def inactive_tools() -> list[str]:
    """Names of registered tools the node cannot call."""
    exposed = {t.name for t in plugin_tools()}
    return [str(getattr(t, "name", "?")) for t in llm_tools.func_list if getattr(t, "name", None) not in exposed]


def find_tool(name: str) -> FunctionTool | None:
    return next((t for t in plugin_tools() if t.name == name), None)


def tool_schema(tool: FunctionTool) -> dict[str, Any]:
    """``{"name", "description", "parameters"}`` for the node."""
    parameters = tool.parameters if isinstance(tool.parameters, dict) and tool.parameters else {"type": "object", "properties": {}}
    return {"name": tool.name, "description": tool.description or tool.name, "parameters": parameters}


def call_kwargs(fn: Any, arguments: dict[str, Any]) -> dict[str, Any]:
    """Keeps only the arguments the callable accepts, unless it takes ``**kwargs``."""
    try:
        params = inspect.signature(fn).parameters.values()
    except (TypeError, ValueError):
        return dict(arguments)
    if any(p.kind == p.VAR_KEYWORD for p in params):
        return dict(arguments)
    names = {p.name for p in params if p.kind in (p.POSITIONAL_OR_KEYWORD, p.KEYWORD_ONLY)}
    return {k: v for k, v in arguments.items() if k in names}


def invoke(tool: FunctionTool, event: Any, context: Any, arguments: dict[str, Any]) -> Any:
    """Starts the tool the way AstrBot's local tool executor does. Returns a coroutine, async generator or value."""
    if tool.handler is not None:
        return tool.handler(event, **call_kwargs(tool.handler, arguments))
    overrides_call = any("call" in ty.__dict__ and ty.__dict__["call"] is not FunctionTool.call for ty in type(tool).mro())
    if overrides_call:
        from astrbot.core.agent.run_context import ContextWrapper
        from astrbot.core.astr_agent_context import AstrAgentContext

        wrapper = ContextWrapper(context=AstrAgentContext(context=context, event=event))
        return tool.call(wrapper, **arguments)
    run = getattr(tool, "run", None)
    if run is not None:
        return run(event, **call_kwargs(run, arguments))
    raise ValueError(f"tool {tool.name} has no handler")


def result_text(value: Any) -> str | None:
    """Text of a tool return value, including MCP ``CallToolResult`` content."""
    if value is None:
        return None
    content = getattr(value, "content", None)
    if isinstance(content, list) and not isinstance(value, (str, bytes)):
        parts = [str(getattr(c, "text", "")) for c in content if getattr(c, "text", None)]
        return "\n".join(parts) if parts else None
    return str(value)
