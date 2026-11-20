from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star


class ToolPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)

    @filter.llm_tool(name="lookup")
    async def lookup(self, event: AstrMessageEvent, q: str, limit: int = 3):
        """Looks a word up.

        Args:
            q(string): the word
                to look up
            limit(number): how many results
        """
        hooked = event.get_extra("hooked")
        return f"{q}:{limit}:{hooked}"

    @filter.llm_tool(name="sticker")
    async def sticker(self, event: AstrMessageEvent, caption: str):
        """Sends a sticker."""
        if caption == "none":
            yield "no such sticker"
            return
        yield event.plain_result(f"[sticker {caption}]")

    @filter.llm_tool(name="boom")
    async def boom(self, event: AstrMessageEvent):
        """Always fails."""
        raise RuntimeError("kaput")

    @filter.on_llm_request()
    async def hook(self, event: AstrMessageEvent, req):
        event.set_extra("hooked", "yes")
