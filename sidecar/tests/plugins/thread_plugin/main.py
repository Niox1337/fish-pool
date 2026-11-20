import threading
import time

from astrbot.api.star import Context, Star

threading.Thread(target=time.sleep, args=(30,), daemon=True).start()


class ThreadPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
