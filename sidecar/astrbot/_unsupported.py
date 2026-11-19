"""Errors and placeholders for AstrBot features the fish sidecar does not provide."""

from __future__ import annotations

import importlib.abc
import importlib.machinery
import sys
import types
from typing import Any, NoReturn


class UnsupportedFeature(ImportError, NotImplementedError):
    """Raised when a plugin uses an AstrBot feature the sidecar does not implement.

    It is both an ImportError and a NotImplementedError, so plugins that guard
    optional imports with ``except ImportError`` keep working.
    """

    def __init__(self, feature: str) -> None:
        super().__init__(f"{feature} is not supported by the fish sidecar")
        self.feature = feature


def unsupported(feature: str) -> NoReturn:
    raise UnsupportedFeature(feature)


class UnsupportedObject:
    """Stands in for an AstrBot service. Any attribute use or call raises UnsupportedFeature."""

    def __init__(self, feature: str) -> None:
        object.__setattr__(self, "_feature", feature)

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        unsupported(f"{self._feature}.{name}")

    def __setattr__(self, name: str, value: Any) -> None:
        unsupported(f"{self._feature}.{name}")

    def __call__(self, *args: Any, **kwargs: Any) -> NoReturn:
        unsupported(self._feature)

    def __repr__(self) -> str:
        return f"<unsupported {self._feature}>"


class _StubModule(types.ModuleType):
    """Module object for an ``astrbot.*`` path the sidecar does not ship."""

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        unsupported(f"{self.__name__}.{name}")


class _StubFinder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    """Resolves unknown ``astrbot.*`` imports to stub modules.

    It sits last on ``sys.meta_path``, so it only sees names the real files do
    not provide. ``import astrbot.x`` then succeeds and ``from astrbot.x import Y``
    raises UnsupportedFeature naming ``astrbot.x.Y``.
    """

    def find_spec(self, fullname: str, path: Any = None, target: Any = None):
        if not fullname.startswith("astrbot."):
            return None
        return importlib.machinery.ModuleSpec(fullname, self, is_package=True)

    def create_module(self, spec: importlib.machinery.ModuleSpec) -> types.ModuleType:
        module = _StubModule(spec.name)
        module.__path__ = []
        return module

    def exec_module(self, module: types.ModuleType) -> None:
        return None


_finder = _StubFinder()


def install_finder() -> None:
    if _finder not in sys.meta_path:
        sys.meta_path.append(_finder)
