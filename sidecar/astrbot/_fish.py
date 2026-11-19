"""Link between the astrbot compatibility package and the fish child process runtime.

The child process registers a runtime object here. Code in the compatibility
package calls it to send messages, call the LLM and locate directories.
"""

from __future__ import annotations

import os
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


def set_runtime(runtime: Runtime | None) -> None:
    global _runtime
    _runtime = runtime


def get_runtime() -> Runtime:
    if _runtime is None:
        raise RuntimeError("the astrbot compatibility package is not connected to a fish node")
    return _runtime


def has_runtime() -> bool:
    return _runtime is not None


def data_dir() -> Path:
    """The writable data directory of the current plugin process."""
    if _runtime is not None:
        return Path(_runtime.data_dir)
    return Path(os.environ.get("FISH_PLUGIN_DATA_DIR") or Path.cwd()).resolve()


def plugin_name() -> str:
    if _runtime is not None:
        return _runtime.plugin_name
    return os.environ.get("FISH_PLUGIN_NAME", "")
