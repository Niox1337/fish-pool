"""Multi turn sessions. A handler waits until later messages of the same session arrive."""

from __future__ import annotations

import abc
import asyncio
import copy
import functools
import time
from collections.abc import Awaitable, Callable
from typing import Any

USER_SESSIONS: dict[str, SessionWaiter] = {}
FILTERS: list[SessionFilter] = []


class SessionController:
    """Lets the waiting function keep the session open or end it."""

    def __init__(self) -> None:
        self.future: asyncio.Future = asyncio.get_running_loop().create_future()
        self.current_event: asyncio.Event | None = None
        self.ts: float | None = None
        self.timeout: float | None = None
        self.history_chains: list[list[Any]] = []

    def stop(self, error: Exception | None = None) -> None:
        if self.future.done():
            return
        if error is not None:
            self.future.set_exception(error)
        else:
            self.future.set_result(None)

    def keep(self, timeout: float = 0, reset_timeout: bool = False) -> None:
        """Extends the session. With reset_timeout the window restarts at ``timeout`` seconds,
        otherwise ``timeout`` is added to the time left."""
        now = time.time()
        if not reset_timeout:
            left = (self.timeout or 0) - (now - (self.ts or now))
            timeout = left + timeout
        if timeout <= 0:
            self.stop()
            return
        if self.current_event is not None:
            self.current_event.set()
        holder = asyncio.Event()
        self.current_event, self.ts, self.timeout = holder, now, timeout
        asyncio.get_running_loop().create_task(self._expire(holder, timeout))

    async def _expire(self, holder: asyncio.Event, timeout: float) -> None:
        try:
            await asyncio.wait_for(holder.wait(), timeout)
        except asyncio.TimeoutError:
            if not self.future.done():
                self.future.set_exception(TimeoutError("等待超时"))

    def get_history_chains(self) -> list[list[Any]]:
        return self.history_chains


class SessionFilter:
    @abc.abstractmethod
    def filter(self, event: Any) -> str:
        """Returns the session key of an event."""


class DefaultSessionFilter(SessionFilter):
    def filter(self, event: Any) -> str:
        return event.unified_msg_origin


class SessionWaiter:
    def __init__(self, session_filter: SessionFilter, session_id: str, record_history_chains: bool) -> None:
        self.session_id = session_id
        self.session_filter = session_filter
        self.record_history_chains = record_history_chains
        self.handler: Callable[[SessionController, Any], Awaitable[Any]] | None = None
        self.session_controller = SessionController()
        self._lock = asyncio.Lock()

    async def register_wait(self, handler: Callable[[SessionController, Any], Awaitable[Any]], timeout: float = 30) -> Any:
        self.handler = handler
        USER_SESSIONS[self.session_id] = self
        self.session_controller.keep(timeout, reset_timeout=True)
        try:
            return await self.session_controller.future
        finally:
            self._cleanup()

    def _cleanup(self) -> None:
        if USER_SESSIONS.get(self.session_id) is self:
            del USER_SESSIONS[self.session_id]
        if self.session_filter in FILTERS:
            FILTERS.remove(self.session_filter)
        self.session_controller.stop()
        if self.session_controller.current_event is not None:
            self.session_controller.current_event.set()

    @classmethod
    async def trigger(cls, session_id: str, event: Any) -> None:
        waiter = USER_SESSIONS.get(session_id)
        if waiter is None or waiter.session_controller.future.done():
            return
        async with waiter._lock:
            controller = waiter.session_controller
            if controller.future.done() or waiter.handler is None:
                return
            if waiter.record_history_chains:
                controller.history_chains.append([copy.deepcopy(c) for c in event.get_messages()])
            try:
                await waiter.handler(controller, event)
            except Exception as e:
                controller.stop(e)


def find_waiting_session(event: Any) -> str | None:
    """Returns the session key when a waiter is waiting for this event."""
    for session_filter in list(FILTERS):
        key = session_filter.filter(event)
        waiter = USER_SESSIONS.get(key)
        if waiter is not None and not waiter.session_controller.future.done():
            return key
    return None


def session_waiter(timeout: int = 30, record_history_chains: bool = False):
    """Decorates ``async def fn(controller, event)``. Calling the result with an event
    waits for following messages of that session and runs ``fn`` for each one."""

    def decorator(func: Callable[[SessionController, Any], Awaitable[Any]]):
        @functools.wraps(func)
        async def wrapper(event: Any, session_filter: SessionFilter | None = None, *args: Any, **kwargs: Any) -> Any:
            session_filter = session_filter or DefaultSessionFilter()
            if not isinstance(session_filter, SessionFilter):
                raise ValueError("session_filter must be a SessionFilter")
            key = session_filter.filter(event)
            FILTERS.append(session_filter)
            waiter = SessionWaiter(session_filter, key, record_history_chains)
            return await waiter.register_wait(func, timeout)

        return wrapper

    return decorator
