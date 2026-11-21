import time as clock

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class SpinPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("spin")
    async def spin(self, event: AstrMessageEvent, seconds: float):
        end = clock.monotonic() + seconds
        while clock.monotonic() < end:
            pass
        yield event.plain_result("spun")

    @filter.command("say")
    async def say(self, event: AstrMessageEvent, text: str):
        print(f"printed {text}")
        yield event.plain_result(text)
