from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

from .lib import VALUE


class TwinPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("twin")
    async def twin(self, event: AstrMessageEvent):
        yield event.plain_result(VALUE)
