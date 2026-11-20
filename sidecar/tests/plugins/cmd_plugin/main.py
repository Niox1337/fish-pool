from astrbot.api import AstrBotConfig, logger
from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star, register
from astrbot.core.agent.tool import ToolSet


@register("cmd_plugin", "fish", "command sample", "1.2.3")
class CmdPlugin(Star):
    def __init__(self, context: Context, config: AstrBotConfig):
        super().__init__(context)
        self.initialized = False
        self.echo_prefix = config.get("echo_prefix", "echo")

    async def initialize(self):
        self.initialized = True

    @filter.command("echo", alias={"复读"})
    async def echo(self, event: AstrMessageEvent, text: str):
        yield event.plain_result(f"{self.echo_prefix}:{text}")

    @filter.command("add")
    async def add(self, event: AstrMessageEvent, a: int, b: int = 10):
        yield event.plain_result(str(a + b))

    @filter.command("whoami")
    async def whoami(self, event: AstrMessageEvent):
        parts = [
            event.get_sender_id(),
            event.get_sender_name(),
            event.get_platform_name(),
            event.get_message_type().name,
            str(event.is_private_chat()),
            event.unified_msg_origin,
            str(self.initialized),
        ]
        await event.send(event.make_result().message("|".join(parts)))
        logger.info("whoami answered")

    @filter.permission_type(filter.PermissionType.ADMIN)
    @filter.command("secret")
    async def secret(self, event: AstrMessageEvent):
        yield event.plain_result("admin only")

    @filter.command("quiet")
    async def quiet(self, event: AstrMessageEvent):
        event.stop_event()

    @filter.llm_tool(name="echo_tool")
    async def echo_tool(self, event: AstrMessageEvent, text: str):
        """Echoes text with the configured prefix.

        Args:
            text(string): text to echo
        """
        return f"{self.echo_prefix}:{text}"

    @filter.command("crash")
    async def crash(self, event: AstrMessageEvent):
        raise RuntimeError("boom")

    @filter.command("ask_tool")
    async def ask_tool(self, event: AstrMessageEvent):
        tools = self.context.get_llm_tool_manager()
        response = await self.context.tool_loop_agent(
            event=event, chat_provider_id="fish", prompt="Please use the echo tool",
            tools=ToolSet([tools.get_func("echo_tool")]), max_steps=3,
        )
        yield event.plain_result(response.completion_text)
