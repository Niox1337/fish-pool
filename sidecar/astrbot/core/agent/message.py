"""Content parts that plugins attach to LLM requests."""

from __future__ import annotations

from typing import Any

from astrbot._unsupported import unsupported


class ContentPart:
    type: str = "text"

    def __init__(self, **kwargs: Any) -> None:
        for key, value in kwargs.items():
            setattr(self, key, value)
        self._no_save = False

    def mark_as_temp(self):
        """Marks the part as belonging to this request only."""
        self._no_save = True
        return self

    def model_dump(self) -> dict[str, Any]:
        data = {k: v for k, v in vars(self).items() if not k.startswith("_")}
        return {"type": self.type, **data}

    def model_dump_for_context(self) -> dict[str, Any]:
        data = self.model_dump()
        if self._no_save:
            data["_no_save"] = True
        return data

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.model_dump()!r})"


class TextPart(ContentPart):
    type = "text"
    text: str = ""

    def __init__(self, text: str = "", **kwargs: Any) -> None:
        super().__init__(text=text, **kwargs)


class ThinkPart(ContentPart):
    type = "think"
    think: str = ""


class ImageURLPart(ContentPart):
    type = "image_url"
    image_url: Any = None


class AudioURLPart(ContentPart):
    type = "audio_url"
    audio_url: Any = None


class Message:
    """Chat message for ``contexts``. Plain dicts with role and content work too."""

    def __init__(self, role: str, content: Any = None, **kwargs: Any) -> None:
        self.role = role
        self.content = content
        for key, value in kwargs.items():
            setattr(self, key, value)

    def model_dump(self) -> dict[str, Any]:
        content = self.content
        if isinstance(content, list):
            content = "".join(getattr(p, "text", "") for p in content)
        return {"role": self.role, "content": content}


def _unsupported_cls(name: str):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        unsupported(f"astrbot.core.agent.message.{name}")

    return type(name, (), {"__init__": __init__})


ToolCall = _unsupported_cls("ToolCall")
ToolCallPart = _unsupported_cls("ToolCallPart")
AssistantMessageSegment = _unsupported_cls("AssistantMessageSegment")
ToolCallMessageSegment = _unsupported_cls("ToolCallMessageSegment")
UserMessageSegment = _unsupported_cls("UserMessageSegment")
SystemMessageSegment = _unsupported_cls("SystemMessageSegment")
