from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

from .lib import VALUE


class RelPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("rel")
    async def rel(self, event: AstrMessageEvent):
        yield event.plain_result(VALUE)
