"""Link between the fish adapters and the child process runtime.

The child process registers a runtime object here. The AstrBot adapters in
``fish_sidecar.adapter`` call it to send messages, call the LLM and start tasks.
"""

from __future__ import annotations

import os
from contextvars import ContextVar
from pathlib import Path
from typing import Any, Protocol


class Runtime(Protocol):
    plugin_name: str
    plugin_dir: Path
    data_dir: Path

    async def send(self, event_id: str | None, session: str, chain: list[dict]) -> None: ...

    async def send_to(self, session: str, chain: list[dict]) -> None: ...

    async def llm(
        self,
        prompt: str | None,
        system_prompt: str | None,
        contexts: list[dict] | None,
        image_urls: list[str] | None,
    ) -> str: ...

    def start_task(self, awaitable: Any, desc: str) -> None: ...


_runtime: Runtime | None = None
# Set inside a group process, where each plugin runs in its own context.
slot: ContextVar[Runtime | None] = ContextVar("fish_slot", default=None)


def current() -> Runtime | None:
    return slot.get() or _runtime


def set_runtime(runtime: Runtime | None) -> None:
    global _runtime
    _runtime = runtime


def get_runtime() -> Runtime:
    rt = current()
    if rt is None:
        raise RuntimeError("the plugin process is not connected to a fish node")
    return rt


def has_runtime() -> bool:
    return current() is not None


def data_dir() -> Path:
    """The writable data directory of the current plugin process."""
    rt = current()
    if rt is not None:
        return Path(rt.data_dir)
    return Path(os.environ.get("FISH_PLUGIN_DATA_DIR") or Path.cwd()).resolve()


def plugin_name() -> str:
    rt = current()
    if rt is not None:
        return rt.plugin_name
    return os.environ.get("FISH_PLUGIN_NAME", "")
