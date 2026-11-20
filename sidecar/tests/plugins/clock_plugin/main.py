import datetime

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class Stats:
    def __init__(self) -> None:
        self.started = datetime.datetime.now()


STATS = Stats()


class ClockPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("uptime")
    async def uptime(self, event: AstrMessageEvent):
        yield event.plain_result(STATS.started.isoformat())
