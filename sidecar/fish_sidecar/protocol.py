"""JSON lines framing shared by the fork server and plugin processes."""

from __future__ import annotations

import json
from typing import Any

MAX_LINE = 32 * 1024 * 1024


def encode(message: dict[str, Any]) -> bytes:
    return json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"


def decode(line: bytes) -> dict[str, Any] | None:
    line = line.strip()
    if not line:
        return None
    value = json.loads(line)
    return value if isinstance(value, dict) else None
