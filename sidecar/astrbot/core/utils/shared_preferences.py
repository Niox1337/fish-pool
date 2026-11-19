"""Key value store kept as one JSON file inside the plugin data directory."""

from __future__ import annotations

import json
import os
import threading
from pathlib import Path
from typing import Any

from astrbot import _fish

_GLOBAL = ("global", "global")


class SharedPreferences:
    """Thread safe JSON file store. Every write rewrites the whole file.

    shortcut: one JSON file rewritten per write, move to sqlite when plugins store
    more than a few MB or write many times per second.
    """

    def __init__(self, filename: str = "fish_kv.json") -> None:
        self._filename = filename
        self._lock = threading.Lock()
        self._cache: dict[str, Any] | None = None
        self._path: Path | None = None

    def _file(self) -> Path:
        return _fish.data_dir() / self._filename

    def _load(self) -> dict[str, Any]:
        path = self._file()
        if self._cache is None or self._path != path:
            try:
                self._cache = json.loads(path.read_text("utf-8"))
            except (OSError, ValueError):
                self._cache = {}
            self._path = path
        return self._cache

    def _save(self) -> None:
        path = self._file()
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self._cache, ensure_ascii=False), "utf-8")
        os.replace(tmp, path)

    @staticmethod
    def _key(scope: str | None, scope_id: str | None, key: str) -> str:
        return f"{scope or _GLOBAL[0]}\x1f{scope_id or _GLOBAL[1]}\x1f{key}"

    def get(self, key: str, default: Any = None, scope: str | None = None, scope_id: str | None = None) -> Any:
        with self._lock:
            return self._load().get(self._key(scope, scope_id, key), default)

    def put(self, key: str, value: Any, scope: str | None = None, scope_id: str | None = None) -> None:
        with self._lock:
            self._load()[self._key(scope, scope_id, key)] = value
            self._save()

    def remove(self, key: str, scope: str | None = None, scope_id: str | None = None) -> None:
        with self._lock:
            if self._load().pop(self._key(scope, scope_id, key), None) is not None:
                self._save()

    def clear(self) -> None:
        with self._lock:
            self._load().clear()
            self._save()

    async def get_async(self, scope: str, scope_id: str, key: str, default: Any = None) -> Any:
        return self.get(key, default, scope, scope_id)

    async def put_async(self, scope: str, scope_id: str, key: str, value: Any) -> None:
        self.put(key, value, scope, scope_id)

    async def remove_async(self, scope: str, scope_id: str, key: str) -> None:
        self.remove(key, scope, scope_id)

    async def range_get_async(self, scope: str, scope_id: str | None = None, key: str | None = None) -> list[Any]:
        with self._lock:
            out = []
            for full, value in self._load().items():
                s, sid, k = full.split("\x1f", 2)
                if s == scope and (scope_id is None or sid == scope_id) and (key is None or k == key):
                    out.append(value)
            return out

    def session_get(self, umo: str | None, key: str, default: Any = None) -> Any:
        return self.get(key, default, "umo", umo or "")

    def session_put(self, umo: str, key: str, value: Any) -> None:
        self.put(key, value, "umo", umo)

    def session_remove(self, umo: str, key: str) -> None:
        self.remove(key, "umo", umo)

    def global_get(self, key: str, default: Any = None) -> Any:
        return self.get(key, default)

    def global_put(self, key: str, value: Any) -> None:
        self.put(key, value)

    def global_remove(self, key: str) -> None:
        self.remove(key)


sp = SharedPreferences()
