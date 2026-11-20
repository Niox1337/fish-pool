import random

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

TOKEN = random.getrandbits(64)


class SeedPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("token")
    async def token(self, event: AstrMessageEvent):
        yield event.plain_result(str(TOKEN))
