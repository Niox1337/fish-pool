from astrbot import logger
from astrbot._unsupported import UnsupportedObject
from astrbot.core.utils.shared_preferences import sp

html_renderer = UnsupportedObject("html_renderer")
file_token_service = UnsupportedObject("file_token_service")
web_chat_queue = UnsupportedObject("web_chat_queue")

__all__ = ["file_token_service", "html_renderer", "logger", "sp", "web_chat_queue"]
