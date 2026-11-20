import gc
import weakref

from astrbot.api.event import AstrMessageEvent, MessageChain, filter
from astrbot.api.star import Star
from astrbot.core.utils.session_waiter import SessionController, session_waiter


class ColdCommandPlugin(Star):
    @filter.command("pond", alias={"鱼池"})
    async def pond(self, event: AstrMessageEvent, text: str):
        self.last_pond_event = weakref.ref(event)
        yield event.plain_result(f"pond:{text}")

    @filter.command("event_released")
    async def event_released(self, event: AstrMessageEvent):
        gc.collect()
        yield event.plain_result(str(self.last_pond_event() is None))

    @filter.command("bait")
    async def bait(self, event: AstrMessageEvent):
        @session_waiter(timeout=10)
        async def wait_for_bait(controller: SessionController, followup: AstrMessageEvent):
            await followup.send(MessageChain().message(f"caught:{followup.message_str}"))
            controller.stop()

        await event.send(MessageChain().message("waiting for bait"))
        await wait_for_bait(event)
