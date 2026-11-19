from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class GroupPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.command_group("math", alias={"m"})
    def math(self):
        pass

    @math.command("add")
    async def math_add(self, event: AstrMessageEvent, a: int, b: int):
        yield event.plain_result(f"sum={a + b}")

    @math.group("adv")
    def adv(self):
        pass

    @adv.command("pow")
    async def adv_pow(self, event: AstrMessageEvent, a: int, b: int):
        yield event.plain_result(f"pow={a ** b}")
