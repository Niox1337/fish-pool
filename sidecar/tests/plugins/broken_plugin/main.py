from astrbot.api.star import Context, Star
from astrbot.core.pipeline.process_stage import Something


class Broken(Star):
    def __init__(self, context: Context):
        super().__init__(context)
