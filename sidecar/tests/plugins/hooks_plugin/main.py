from mcp.types import CallToolResult, ImageContent, TextContent

from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.message_components import Image
from astrbot.api.provider import LLMResponse
from astrbot.api.star import Context, Star


class HooksPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
        self.seen: list[str] = []

    @filter.command("seen")
    async def show(self, event: AstrMessageEvent):
        yield event.plain_result(";".join(self.seen))
        self.seen.clear()

    @filter.command("boom")
    async def boom(self, event: AstrMessageEvent):
        raise RuntimeError("kaboom")
        yield

    @filter.on_agent_begin()
    async def begin(self, event: AstrMessageEvent, run_context):
        event.set_extra("run", run_context)
        self.seen.append(f"begin:{len(run_context.messages)}")

    @filter.on_using_llm_tool()
    async def using(self, event: AstrMessageEvent, tool, tool_args):
        self.seen.append(f"using:{tool.name}:{tool_args.get('q')}")

    @filter.on_llm_tool_respond()
    async def responded(self, event: AstrMessageEvent, tool, tool_args, tool_result):
        self.seen.append(f"respond:{tool.name}:{tool_result.content[0].text}")

    @filter.on_llm_response()
    async def response(self, event: AstrMessageEvent, resp: LLMResponse):
        if "[tag]" in resp.completion_text:
            resp.completion_text = resp.completion_text.replace("[tag]", "")
            event.set_extra("tagged", True)

    @filter.on_agent_done()
    async def done(self, event: AstrMessageEvent, run_context, resp: LLMResponse):
        self.seen.append(f"done:{run_context is event.get_extra('run')}:{resp.completion_text}")

    @filter.on_decorating_result()
    async def decorate(self, event: AstrMessageEvent):
        result = event.get_result()
        if event.get_extra("tagged") and result.is_llm_result():
            result.chain.append(Image.fromURL("https://example.com/m.png"))

    @filter.after_message_sent()
    async def sent(self, event: AstrMessageEvent):
        self.seen.append(f"sent:{len(event.get_result().chain)}")

    @filter.on_plugin_error()
    async def on_error(self, event: AstrMessageEvent, plugin_name, handler_name, error, traceback_text):
        self.seen.append(f"error:{plugin_name}:{handler_name}:{error}")

    @filter.on_plugin_loaded()
    async def loaded(self, metadata):
        self.seen.append(f"loaded:{metadata.name}")

    @filter.on_plugin_unloaded()
    async def unloaded(self, metadata):
        self.seen.append(f"unloaded:{metadata.name}")

    @filter.llm_tool(name="picture")
    async def picture(self, event: AstrMessageEvent, q: str):
        """Draws a picture.

        Args:
            q(string): what to draw
        """
        return CallToolResult(
            content=[
                TextContent(type="text", text=f"drew {q}"),
                ImageContent(type="image", data="AAAA", mimeType="image/png"),
            ]
        )
