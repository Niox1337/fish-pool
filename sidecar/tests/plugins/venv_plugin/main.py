import fishvenvdep

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class VenvPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("dep")
    async def dep(self, event: AstrMessageEvent):
        yield event.plain_result(fishvenvdep.VALUE)
