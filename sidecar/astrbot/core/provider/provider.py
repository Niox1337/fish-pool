"""Provider classes. The only working chat provider forwards to the fish node ``llm`` op."""

from __future__ import annotations

from typing import Any

from astrbot import _fish
from astrbot._unsupported import unsupported
from astrbot.core.provider.entities import LLMResponse, ProviderMeta, ProviderType


class LLMError(RuntimeError):
    """The node reported an error for an LLM call."""


def _context_dicts(contexts: list | None) -> list[dict] | None:
    if not contexts:
        return None
    out = []
    for item in contexts:
        if isinstance(item, dict):
            out.append({"role": item.get("role", "user"), "content": item.get("content", "")})
        elif hasattr(item, "model_dump"):
            out.append(item.model_dump())
        else:
            out.append({"role": getattr(item, "role", "user"), "content": getattr(item, "content", "")})
    return out


class AbstractProvider:
    def __init__(self, provider_config: dict | None = None, *args: Any, **kwargs: Any) -> None:
        self.provider_config = provider_config or {}
        self.model_name = self.provider_config.get("model", "")

    def get_model(self) -> str:
        return self.model_name

    def set_model(self, model_name: str) -> None:
        self.model_name = model_name

    def meta(self) -> ProviderMeta:
        return ProviderMeta(id="fish", model=self.model_name, type="fish", provider_type=ProviderType.CHAT_COMPLETION)


class Provider(AbstractProvider):
    """Chat completion provider base."""

    async def text_chat(self, prompt: str | None = None, **kwargs: Any) -> LLMResponse:
        unsupported(f"{type(self).__name__}.text_chat")

    async def text_chat_stream(self, *args: Any, **kwargs: Any):
        response = await self.text_chat(*args, **kwargs)
        yield response

    async def get_models(self) -> list[str]:
        return [self.model_name] if self.model_name else []

    async def terminate(self) -> None:
        return None


class FishProvider(Provider):
    """Calls the bot's LLM through the fish node."""

    def __init__(self) -> None:
        super().__init__({"id": "fish", "model": ""})

    async def text_chat(
        self,
        prompt: str | None = None,
        session_id: str | None = None,
        image_urls: list[str] | None = None,
        audio_urls: list[str] | None = None,
        func_tool: Any = None,
        contexts: list | None = None,
        system_prompt: str | None = None,
        tool_calls_result: Any = None,
        model: str | None = None,
        extra_user_content_parts: list | None = None,
        **kwargs: Any,
    ) -> LLMResponse:
        if extra_user_content_parts:
            texts = [getattr(p, "text", "") for p in extra_user_content_parts]
            prompt = "\n".join([prompt or "", *[t for t in texts if t]]).strip("\n")
        text = await _fish.get_runtime().llm(prompt, system_prompt or None, _context_dicts(contexts), image_urls or None)
        return LLMResponse(role="assistant", completion_text=text)


class STTProvider(AbstractProvider):
    async def get_text(self, audio_url: str) -> str:
        unsupported("STTProvider.get_text")


class TTSProvider(AbstractProvider):
    async def get_audio(self, text: str) -> str:
        unsupported("TTSProvider.get_audio")


class EmbeddingProvider(AbstractProvider):
    async def get_embedding(self, text: str) -> list[float]:
        unsupported("EmbeddingProvider.get_embedding")


class RerankProvider(AbstractProvider):
    async def rerank(self, *args: Any, **kwargs: Any) -> Any:
        unsupported("RerankProvider.rerank")


class DecisionProvider(AbstractProvider):
    async def decide(self, *args: Any, **kwargs: Any) -> Any:
        unsupported("DecisionProvider.decide")
