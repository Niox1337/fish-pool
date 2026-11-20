import os

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

HAS_SETTINGS = os.path.exists("settings.json")


class CwdPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("cwd")
    async def cwd(self, event: AstrMessageEvent):
        yield event.plain_result(str(HAS_SETTINGS))
