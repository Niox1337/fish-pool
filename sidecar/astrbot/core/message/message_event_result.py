"""Message chains and handler results."""

from __future__ import annotations

import enum
import logging
from collections.abc import AsyncGenerator
from dataclasses import dataclass, field

from astrbot.core.message.components import At, AtAll, BaseMessageComponent, Image, Json, Plain


@dataclass
class MessageChain:
    """An ordered list of components that forms one rich message."""

    chain: list[BaseMessageComponent] = field(default_factory=list)
    use_t2i_: bool | None = None
    use_markdown_: bool | None = None
    type: str | None = None

    def derive(self, chain: list[BaseMessageComponent] | None = None) -> MessageChain:
        new = MessageChain(chain=chain if chain is not None else [])
        new.use_t2i_ = self.use_t2i_
        new.use_markdown_ = self.use_markdown_
        new.type = self.type
        return new

    def message(self, message: str):
        self.chain.append(Plain(message))
        return self

    def at(self, name: str, qq: str | int):
        self.chain.append(At(name=name, qq=qq))
        return self

    def at_all(self):
        self.chain.append(AtAll())
        return self

    def error(self, message: str):
        self.chain.append(Plain(message))
        return self

    def url_image(self, url: str):
        self.chain.append(Image.fromURL(url))
        return self

    def file_image(self, path: str):
        self.chain.append(Image.fromFileSystem(path))
        return self

    def base64_image(self, base64_str: str):
        self.chain.append(Image.fromBase64(base64_str))
        return self

    def use_t2i(self, use_t2i: bool):
        self.use_t2i_ = use_t2i
        return self

    def use_markdown(self, use: bool | None = True):
        self.use_markdown_ = use
        return self

    def get_plain_text(self, with_other_comps_mark: bool = False) -> str:
        parts: list[str] = []
        for comp in self.chain:
            if isinstance(comp, Plain):
                parts.append(comp.text)
            elif with_other_comps_mark:
                parts.append(str(comp.data) if isinstance(comp, Json) else f"[{type(comp).__name__}]")
        return " ".join(parts)

    def squash_plain(self):
        if not self.chain:
            return None
        squashed: list[BaseMessageComponent] = []
        first: Plain | None = None
        texts: list[str] = []
        for comp in self.chain:
            if isinstance(comp, Plain):
                if first is None:
                    first = comp
                    squashed.append(comp)
                texts.append(comp.text)
            else:
                squashed.append(comp)
        if first is not None:
            first.text = "".join(texts)
        self.chain = squashed
        return self


class EventResultType(enum.Enum):
    CONTINUE = enum.auto()
    STOP = enum.auto()


class ResultContentType(enum.Enum):
    LLM_RESULT = enum.auto()
    AGENT_RUNNER_ERROR = enum.auto()
    GENERAL_RESULT = enum.auto()
    STREAMING_RESULT = enum.auto()
    STREAMING_FINISH = enum.auto()


@dataclass
class MessageEventResult(MessageChain):
    """A handler result: a message chain plus propagation and content flags."""

    result_type: EventResultType | None = field(default_factory=lambda: EventResultType.CONTINUE)
    result_content_type: ResultContentType | None = field(
        default_factory=lambda: ResultContentType.GENERAL_RESULT
    )
    async_stream: AsyncGenerator | None = None

    def stop_event(self) -> MessageEventResult:
        self.result_type = EventResultType.STOP
        return self

    def continue_event(self) -> MessageEventResult:
        self.result_type = EventResultType.CONTINUE
        return self

    def is_stopped(self) -> bool:
        return self.result_type == EventResultType.STOP

    def set_result_type(self, typ: EventResultType) -> MessageEventResult:
        self.result_type = typ
        return self

    def set_console_log(self, log: str, level: int = logging.INFO) -> MessageEventResult:
        logging.getLogger("astrbot").log(level, log)
        return self

    def set_async_stream(self, stream: AsyncGenerator) -> MessageEventResult:
        self.async_stream = stream
        return self

    def set_result_content_type(self, typ: ResultContentType) -> MessageEventResult:
        self.result_content_type = typ
        return self

    def is_llm_result(self) -> bool:
        return self.result_content_type == ResultContentType.LLM_RESULT

    def is_model_result(self) -> bool:
        return self.result_content_type in (
            ResultContentType.LLM_RESULT,
            ResultContentType.AGENT_RUNNER_ERROR,
        )


CommandResult = MessageEventResult

__all__ = [
    "CommandResult",
    "EventResultType",
    "MessageChain",
    "MessageEventResult",
    "ResultContentType",
]
