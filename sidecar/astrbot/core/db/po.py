"""Plain records that appear in provider and conversation signatures."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class Conversation:
    platform_id: str = ""
    user_id: str = ""
    cid: str = ""
    history: str = "[]"
    title: str | None = ""
    persona_id: str | None = ""
    created_at: int = 0
    updated_at: int = 0


@dataclass
class Personality:
    prompt: str = ""
    name: str = ""
    begin_dialogs: list[str] = field(default_factory=list)
    mood_imitation_dialogs: list[str] = field(default_factory=list)
    tools: list[str] | None = None
    skills: list[str] | None = None
    extra: dict[str, Any] = field(default_factory=dict)
