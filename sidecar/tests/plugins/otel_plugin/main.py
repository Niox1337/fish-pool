import uuid

from opentelemetry.context import create_key

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star

KEY = create_key("fish-test")


class OtelPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command("key")
    async def key(self, event: AstrMessageEvent):
        yield event.plain_result(f"{KEY}|{uuid.uuid4()}")
