from astrbot.api import AstrBotConfig
from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.provider import ProviderRequest
from astrbot.api.star import Context, Star, StarTools
from astrbot.core.agent.message import TextPart


class ConfPlugin(Star):
    def __init__(self, context: Context, config: AstrBotConfig):
        super().__init__(context)
        self.config = config

    @filter.command("conf")
    async def conf(self, event: AstrMessageEvent):
        nested = self.config.get("nested", {})
        yield event.plain_result(
            f"{self.config['greeting']}|{self.config.get('times')}|{self.config.get('enabled')}|{nested.get('level')}"
        )

    @filter.command("kv")
    async def kv(self, event: AstrMessageEvent, value: str):
        old = await self.get_kv_data("last", "none")
        await self.put_kv_data("last", value)
        data_dir = StarTools.get_data_dir()
        yield event.plain_result(f"{old}->{value}|{data_dir.name}|{data_dir.exists()}")

    @filter.on_llm_request(priority=5)
    async def inject(self, event: AstrMessageEvent, req: ProviderRequest):
        req.system_prompt = f"{self.config['greeting']} {req.system_prompt}".strip()
        req.extra_user_content_parts.append(TextPart(text="[extra]"))

    @filter.on_decorating_result()
    async def decorate(self, event: AstrMessageEvent):
        result = event.get_result()
        if result is not None and result.chain:
            result.chain.append(__import__("astrbot.api.message_components", fromlist=["Plain"]).Plain("~"))
