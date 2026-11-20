import os
import threading

from astrbot.api import AstrBotConfig, logger
from astrbot.api.event import AstrMessageEvent, filter
from astrbot.api.star import Context, Star, StarTools

logger.info("share plugin imported")
print("share plugin printed")
IMPORT_THREAD = threading.current_thread().name


class SharePlugin(Star):
    def __init__(self, context: Context, config: AstrBotConfig):
        super().__init__(context)
        self.config = config

    @filter.command("where")
    async def where(self, event: AstrMessageEvent):
        data_dir = StarTools.get_data_dir()
        yield event.plain_result(
            f"{os.getcwd()}|{os.environ['HOME']}|{data_dir}|{self.config.get('name')}|{IMPORT_THREAD == 'fish-template-import'}"
        )
