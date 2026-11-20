import asyncio

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class BgPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
        self.task: asyncio.Task | None = None

    @filter.command("bgstart")
    async def bgstart(self, event: AstrMessageEvent):
        self.task = asyncio.create_task(self._tick())
        yield event.plain_result("started")

    @filter.command("bgstop")
    async def bgstop(self, event: AstrMessageEvent):
        if self.task is not None:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
            self.task = None
        yield event.plain_result("stopped")

    async def _tick(self):
        while True:
            await asyncio.sleep(3600)
