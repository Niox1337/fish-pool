from astrbot.api import logger
from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.provider import LLMResponse, ProviderRequest
from astrbot.api.star import Context, Star
from astrbot.core.star.filter.command import GreedyStr


class LlmPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("ask")
    async def ask(self, event: AstrMessageEvent, question: GreedyStr):
        yield event.request_llm(prompt=str(question), system_prompt="be short")

    @filter.command("chat")
    async def chat(self, event: AstrMessageEvent, question: GreedyStr):
        provider = self.context.get_using_provider()
        resp = await provider.text_chat(
            prompt=str(question),
            system_prompt="sys",
            contexts=[{"role": "user", "content": "earlier"}],
        )
        yield event.plain_result("chat:" + resp.completion_text)

    @filter.command("fail")
    async def fail(self, event: AstrMessageEvent):
        provider = self.context.get_using_provider()
        try:
            await provider.text_chat(prompt="please fail")
        except Exception as e:
            yield event.plain_result(f"failed:{type(e).__name__}")

    @filter.on_llm_request()
    async def add_persona(self, event: AstrMessageEvent, req: ProviderRequest):
        req.system_prompt = (req.system_prompt or "") + "|hooked"
        event.set_extra("hooked", True)

    @filter.on_llm_response()
    async def tag_response(self, event: AstrMessageEvent, resp: LLMResponse):
        resp.completion_text = resp.completion_text + "!"
        logger.info("response tagged")
