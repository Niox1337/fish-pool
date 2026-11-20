"""Function tool schemas for ``@filter.llm_tool`` handlers.

The description and parameters come from a Google style docstring::

    \"\"\"Searches memes.

    Args:
        q(string): keyword
        limit(number): how many results
    \"\"\"

Parameters without a docstring entry are taken from the signature. Signature parameters
without a default value are required.
"""

from __future__ import annotations

import inspect
import re
from typing import Any

JSON_TYPES = {"string", "number", "integer", "boolean", "array", "object"}
TYPE_ALIASES = {
    "str": "string",
    "int": "integer",
    "float": "number",
    "bool": "boolean",
    "list": "array",
    "dict": "object",
}
TOOL_NAME = re.compile(r"^[A-Za-z0-9_-]{1,64}$")

_ARGS_SECTION = re.compile(r"^(Args|Arguments|Parameters|Params)\s*:\s*$", re.I)
_OTHER_SECTION = re.compile(r"^(Returns?|Yields?|Raises?|Examples?|Notes?|Attributes)\s*:\s*$", re.I)
_PARAM = re.compile(r"^(\w+)\s*(?:\(([^)]*)\))?\s*:\s*(.*)$")


def json_type(name: str | None) -> dict[str, Any]:
    """Maps a docstring or annotation type name to a JSON schema. Unknown names become string."""
    t = (name or "").strip().lower()
    m = re.fullmatch(r"(?:array|list)\s*\[(.+)\]", t)
    if m:
        return {"type": "array", "items": json_type(m.group(1))}
    t = TYPE_ALIASES.get(t, t)
    return {"type": t if t in JSON_TYPES else "string"}


def parse_docstring(doc: str | None) -> tuple[str, dict[str, dict[str, Any]]]:
    """Returns the description and the documented parameters as JSON schemas."""
    lines = inspect.cleandoc(doc or "").splitlines()
    desc: list[str] = []
    params: dict[str, dict[str, Any]] = {}
    section = "desc"
    param_indent: int | None = None
    last: str | None = None
    for raw in lines:
        stripped = raw.strip()
        indent = len(raw) - len(raw.lstrip())
        if _ARGS_SECTION.match(stripped):
            section, param_indent, last = "args", None, None
            continue
        if _OTHER_SECTION.match(stripped):
            section = "other"
            continue
        if section == "desc":
            desc.append(stripped)
        elif section == "args" and stripped:
            m = _PARAM.match(stripped)
            if m and (param_indent is None or indent <= param_indent):
                param_indent = indent
                name, type_name, text = m.groups()
                schema = json_type(type_name)
                if text.strip():
                    schema["description"] = text.strip()
                params[name] = schema
                last = name
            elif last is not None:
                prev = params[last].get("description", "")
                params[last]["description"] = f"{prev} {stripped}".strip()
    return "\n".join(desc).strip(), params


def tool_schema(name: str, fn: Any) -> dict[str, Any]:
    """Builds ``{"name", "description", "parameters"}`` for a tool handler."""
    description, documented = parse_docstring(getattr(fn, "__doc__", None))
    properties: dict[str, dict[str, Any]] = {}
    required: list[str] = []
    try:
        sig_params = list(inspect.signature(fn).parameters.values())
    except (TypeError, ValueError):
        sig_params = []
    positional = [p for p in sig_params if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)]
    if positional and positional[0].name == "self":
        positional = positional[1:]
    skip = {positional[0].name} if positional else set()
    for p in sig_params:
        if p.name in skip or p.name == "self" or p.kind in (p.VAR_POSITIONAL, p.VAR_KEYWORD):
            continue
        if p.name in documented:
            properties[p.name] = documented[p.name]
        else:
            ann = p.annotation
            ann_name = getattr(ann, "__name__", ann if isinstance(ann, str) else None)
            properties[p.name] = json_type(ann_name)
        if p.default is inspect.Parameter.empty:
            required.append(p.name)
    if not sig_params:
        properties = documented
    parameters: dict[str, Any] = {"type": "object", "properties": properties}
    if required:
        parameters["required"] = required
    return {"name": name, "description": description or name, "parameters": parameters}


def call_kwargs(fn: Any, arguments: dict[str, Any]) -> dict[str, Any]:
    """Keeps only the arguments the handler accepts, unless it takes ``**kwargs``."""
    try:
        params = inspect.signature(fn).parameters.values()
    except (TypeError, ValueError):
        return dict(arguments)
    if any(p.kind == p.VAR_KEYWORD for p in params):
        return dict(arguments)
    names = {p.name for p in params if p.kind in (p.POSITIONAL_OR_KEYWORD, p.KEYWORD_ONLY)}
    return {k: v for k, v in arguments.items() if k in names}
