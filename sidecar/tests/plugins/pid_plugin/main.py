import gc
import os

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

OWNER_PID = os.getpid()


class PidPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("pid")
    async def pid(self, event: AstrMessageEvent):
        yield event.plain_result(f"{OWNER_PID}|{os.getpid()}")

    @filter.command("frozen")
    async def frozen(self, event: AstrMessageEvent):
        yield event.plain_result(str(gc.get_freeze_count()))
