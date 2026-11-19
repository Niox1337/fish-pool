import re

import astrbot.api.message_components as Comp
from astrbot.api.event import AstrMessageEvent, MessageChain, filter
from astrbot.api.star import Context, Star


class RegexPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
        self.seen = 0

    @filter.regex(r"^ping(\d*)$")
    async def ping(self, event: AstrMessageEvent):
        m = re.match(r"^ping(\d*)$", event.message_str)
        yield event.plain_result("pong" + (m.group(1) if m else ""))
        yield event.chain_result([Comp.At(qq=event.get_sender_id()), Comp.Plain(" twice")])

    @filter.event_message_type(filter.EventMessageType.GROUP_MESSAGE)
    async def count_group(self, event: AstrMessageEvent):
        self.seen += 1

    @filter.command("push")
    async def push(self, event: AstrMessageEvent):
        await self.context.send_message(event.unified_msg_origin, MessageChain().message("pushed"))
        event.stop_event()
