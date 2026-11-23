"""Returns freed heap memory of the current process to the operating system."""

from __future__ import annotations

import asyncio
import ctypes
import ctypes.util
import gc
import os
import sys
import threading
import time
from typing import Any

# Seconds without plugin messages before an idle process collects garbage and trims its heap.
IDLE_TRIM_S = 60.0
CHECK_INTERVAL_S = 15.0
# Free heap bytes below which a trim is skipped. Trimming walks every free chunk, and in a
# forked process that writes to pages still shared with the parent.
MIN_FREE_BYTES = 8 * 1024 * 1024


class _Mallinfo2(ctypes.Structure):
    _fields_ = [
        (name, ctypes.c_size_t)
        for name in (
            "arena",
            "ordblks",
            "smblks",
            "hblks",
            "hblkhd",
            "usmblks",
            "fsmblks",
            "uordblks",
            "fordblks",
            "keepcost",
        )
    ]


_libc: Any = None
_lock = threading.Lock()
_last_activity = time.monotonic()
_trimmed_after: float | None = None
_started_pid: int | None = None


def _glibc() -> Any:
    """The C library when it provides ``malloc_trim`` and ``mallinfo2``, otherwise None."""
    global _libc
    if _libc is None:
        _libc = False
        if sys.platform.startswith("linux"):
            try:
                lib = ctypes.CDLL(ctypes.util.find_library("c") or "libc.so.6")
                lib.malloc_trim.argtypes = [ctypes.c_size_t]
                lib.malloc_trim.restype = ctypes.c_int
                lib.mallinfo2.restype = _Mallinfo2
                _libc = lib
            except (OSError, AttributeError):
                pass
    return _libc or None


def free_heap_bytes() -> int:
    """Bytes held by malloc in free chunks across all arenas. 0 when unknown."""
    lib = _glibc()
    if lib is None:
        return 0
    return int(lib.mallinfo2().fordblks)


def trim() -> bool:
    """Returns free malloc memory to the operating system. False when glibc is not available."""
    lib = _glibc()
    if lib is None:
        return False
    lib.malloc_trim(0)
    return True


def touch() -> None:
    """Records plugin activity, which delays the next idle trim."""
    global _last_activity
    _last_activity = time.monotonic()


def collect_and_trim(min_free: int = MIN_FREE_BYTES) -> bool:
    """Runs a full garbage collection, then trims the heap when it holds at least ``min_free`` free bytes. True when it trimmed."""
    gc.collect()
    if free_heap_bytes() < min_free:
        return False
    return trim()


def _idle_trim() -> None:
    global _trimmed_after
    with _lock:
        activity = _last_activity
        if _trimmed_after == activity or time.monotonic() - activity < IDLE_TRIM_S:
            return
        _trimmed_after = activity
    collect_and_trim()


def start(loop: asyncio.AbstractEventLoop) -> None:
    """Starts a thread that trims the heap on ``loop`` after ``IDLE_TRIM_S`` seconds without activity. Once per process."""
    global _started_pid
    if _started_pid is not None and _started_pid == os.getpid():
        return
    if _glibc() is None:
        return
    _started_pid = os.getpid()
    touch()

    def watch() -> None:
        while not loop.is_closed():
            time.sleep(CHECK_INTERVAL_S)
            if time.monotonic() - _last_activity < IDLE_TRIM_S or _trimmed_after == _last_activity:
                continue
            try:
                loop.call_soon_threadsafe(_idle_trim)
            except RuntimeError:
                return

    thread = threading.Thread(target=watch, name="fish-memory", daemon=True)
    thread.fish_internal = True  # type: ignore[attr-defined]
    thread.start()
