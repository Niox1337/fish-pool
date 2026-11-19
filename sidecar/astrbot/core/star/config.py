"""Legacy namespaced config files, stored under ``<data_dir>/config``."""

from __future__ import annotations

import json
import os
from typing import Any

from astrbot.core.utils.astrbot_path import get_astrbot_config_path


def _path(namespace: str) -> str:
    return os.path.join(get_astrbot_config_path(), f"{namespace}.json")


def load_config(namespace: str) -> dict | bool:
    path = _path(namespace)
    if not os.path.exists(path):
        return False
    with open(path, encoding="utf-8-sig") as f:
        data = json.load(f)
    return {key: item["value"] for key, item in data.items()}


def put_config(namespace: str, name: str, key: str, value: Any, description: str) -> None:
    if not namespace or namespace.startswith("internal_"):
        raise ValueError("invalid namespace")
    path = _path(namespace)
    data: dict = {}
    if os.path.exists(path):
        with open(path, encoding="utf-8-sig") as f:
            data = json.load(f)
    if key in data:
        return
    data[key] = {
        "config_type": "item",
        "name": name,
        "description": description,
        "path": key,
        "value": value,
        "val_type": type(value).__name__,
    }
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)


def update_config(namespace: str, key: str, value: Any) -> None:
    path = _path(namespace)
    if not os.path.exists(path):
        raise FileNotFoundError(f"{namespace}.json")
    with open(path, encoding="utf-8-sig") as f:
        data = json.load(f)
    if key not in data:
        raise KeyError(key)
    data[key]["value"] = value
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)


__all__ = ["load_config", "put_config", "update_config"]
