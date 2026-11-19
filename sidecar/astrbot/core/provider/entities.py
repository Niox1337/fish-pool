"""LLM request and response records."""

from __future__ import annotations

import enum
from dataclasses import dataclass, field
from typing import Any

from astrbot.core.agent.message import ContentPart
from astrbot.core.agent.tool import ToolSet
from astrbot.core.db.po import Conversation
from astrbot.core.message.components import Plain
from astrbot.core.message.message_event_result import MessageChain


class ProviderType(enum.Enum):
    CHAT_COMPLETION = "chat_completion"
    SPEECH_TO_TEXT = "speech_to_text"
    TEXT_TO_SPEECH = "text_to_speech"
    EMBEDDING = "embedding"
    RERANK = "rerank"
    DECISION = "decision"


@dataclass
class ProviderMeta:
    id: str
    model: str | None
    type: str
    provider_type: ProviderType = ProviderType.CHAT_COMPLETION


@dataclass
class ProviderMetaData(ProviderMeta):
    desc: str = ""
    cls_type: Any = None
    default_config_tmpl: dict | None = None
    provider_display_name: str | None = None


@dataclass
class ProviderRequest:
    """An LLM call that the pipeline runs after the handler yields it."""

    prompt: str | None = None
    session_id: str | None = ""
    image_urls: list[str] = field(default_factory=list)
    audio_urls: list[str] = field(default_factory=list)
    extra_user_content_parts: list[ContentPart] = field(default_factory=list)
    func_tool: ToolSet | None = None
    contexts: list[dict] = field(default_factory=list)
    system_prompt: str = ""
    conversation: Conversation | None = None
    tool_calls_result: Any = None
    model: str | None = None

    def full_prompt(self) -> str | None:
        """The prompt followed by the text of ``extra_user_content_parts``."""
        extra = [getattr(p, "text", "") for p in self.extra_user_content_parts]
        extra = [t for t in extra if t]
        if not extra:
            return self.prompt
        return "\n".join([self.prompt or "", *extra]).strip("\n")


@dataclass
class TokenUsage:
    input_other: int = 0
    input_cached: int = 0
    output: int = 0

    @property
    def total(self) -> int:
        return self.input_other + self.input_cached + self.output

    @property
    def input(self) -> int:
        return self.input_other + self.input_cached


class LLMResponse:
    """Result of an LLM call. ``completion_text`` follows ``result_chain`` when one is set."""

    def __init__(
        self,
        role: str = "assistant",
        completion_text: str | None = None,
        result_chain: MessageChain | None = None,
        tools_call_args: list[dict[str, Any]] | None = None,
        tools_call_name: list[str] | None = None,
        tools_call_ids: list[str] | None = None,
        tools_call_extra_content: dict[str, dict[str, Any]] | None = None,
        reasoning_content: str | None = None,
        reasoning_signature: str | None = None,
        raw_completion: Any = None,
        is_chunk: bool = False,
        id: str | None = None,
        usage: TokenUsage | None = None,
    ) -> None:
        self._completion_text = ""
        self.role = role
        self.result_chain = result_chain
        self.completion_text = completion_text or ""
        self.tools_call_args = tools_call_args or []
        self.tools_call_name = tools_call_name or []
        self.tools_call_ids = tools_call_ids or []
        self.tools_call_extra_content = tools_call_extra_content or {}
        self.reasoning_content = reasoning_content
        self.reasoning_signature = reasoning_signature
        self.raw_completion = raw_completion
        self.is_chunk = is_chunk
        self.id = id
        self.usage = usage

    @property
    def completion_text(self) -> str:
        if self.result_chain:
            return self.result_chain.get_plain_text()
        return self._completion_text

    @completion_text.setter
    def completion_text(self, value: str) -> None:
        if self.result_chain:
            rest = [c for c in self.result_chain.chain if not isinstance(c, Plain)]
            self.result_chain.chain = [Plain(value), *rest]
        else:
            self._completion_text = value

    def __repr__(self) -> str:
        return f"LLMResponse(role={self.role!r}, completion_text={self.completion_text!r})"


@dataclass
class RerankResult:
    index: int
    relevance_score: float


class _Decision:
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        self.args = args
        self.kwargs = kwargs


class DecisionChoiceQuestion(_Decision): ...


class DecisionBooleanQuestion(_Decision): ...


class DecisionScoreQuestion(_Decision): ...


class DecisionChoiceAnswer(_Decision): ...


class DecisionBooleanAnswer(_Decision): ...


class DecisionScoreAnswer(_Decision): ...


class DecisionError(RuntimeError): ...


DecisionQuestion = DecisionChoiceQuestion | DecisionBooleanQuestion | DecisionScoreQuestion
DecisionAnswer = DecisionChoiceAnswer | DecisionBooleanAnswer | DecisionScoreAnswer
DecisionState = Any
