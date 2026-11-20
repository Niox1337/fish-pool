import time

from astrbot.api.star import Context, Star

time.sleep(1)


class SlowPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
