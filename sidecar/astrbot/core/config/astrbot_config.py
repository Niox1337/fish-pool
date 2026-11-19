"""Dictionary based configuration object with attribute access and schema defaults."""

from __future__ import annotations

import copy
import json
import logging
from typing import Any

DEFAULT_VALUE_MAP: dict[str, Any] = {
    "int": 0,
    "float": 0.0,
    "bool": False,
    "string": "",
    "text": "",
    "list": [],
    "file": [],
    "object": {},
    "dict": {},
    "template_list": [],
}

_INTERNAL = ("config_path", "default_config", "schema")


def schema_defaults(schema: dict | None) -> dict:
    """Builds a config dict from the ``default`` values of a ``_conf_schema.json`` tree."""
    conf: dict[str, Any] = {}
    for key, spec in (schema or {}).items():
        if not isinstance(spec, dict):
            continue
        typ = spec.get("type", "string")
        if typ == "object":
            conf[key] = schema_defaults(spec.get("items") or {})
            if isinstance(spec.get("default"), dict):
                conf[key] = deep_merge(conf[key], spec["default"])
        elif "default" in spec:
            conf[key] = copy.deepcopy(spec["default"])
        else:
            conf[key] = copy.deepcopy(DEFAULT_VALUE_MAP.get(typ, ""))
    return conf


def deep_merge(base: dict, override: dict | None) -> dict:
    """Returns ``base`` updated with ``override``. Nested dicts merge key by key."""
    out = copy.deepcopy(base)
    for key, value in (override or {}).items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = deep_merge(out[key], value)
        else:
            out[key] = copy.deepcopy(value)
    return out


class AstrBotConfig(dict):
    """Configuration dict. Missing attributes read as None, attribute writes set keys."""

    def __init__(
        self,
        config_path: str = "",
        default_config: dict | None = None,
        schema: dict | None = None,
        values: dict | None = None,
    ) -> None:
        super().__init__()
        object.__setattr__(self, "config_path", config_path)
        object.__setattr__(self, "schema", schema)
        defaults = schema_defaults(schema) if schema else copy.deepcopy(default_config or {})
        object.__setattr__(self, "default_config", defaults)
        self.update(deep_merge(defaults, values))

    def replace_values(self, values: dict | None) -> None:
        """Replaces the current values with schema defaults merged with ``values``."""
        merged = deep_merge(self.default_config, values)
        self.clear()
        self.update(merged)

    def check_config_integrity(self, refer_conf: dict, conf: dict, path: str = "") -> bool:
        changed = False
        for key, value in refer_conf.items():
            if key not in conf:
                conf[key] = copy.deepcopy(value)
                changed = True
            elif isinstance(value, dict) and isinstance(conf[key], dict):
                changed = self.check_config_integrity(value, conf[key], f"{path}.{key}") or changed
        return changed

    def save_config(self, replace_config: dict | None = None) -> None:
        """Keeps changes in memory for the life of the process.

        shortcut: plugin side config writes are not sent to the console, add a
        node op that persists them when plugins need durable self edited config.
        """
        if replace_config is not None:
            self.clear()
            self.update(replace_config)
        logging.getLogger("astrbot").debug("save_config keeps plugin config in memory only")

    def check_exist(self) -> bool:
        return False

    def to_json(self) -> str:
        return json.dumps(self, ensure_ascii=False)

    def __getattr__(self, item: str) -> Any:
        if item.startswith("__") and item.endswith("__"):
            raise AttributeError(item)
        try:
            return self[item]
        except KeyError:
            return None

    def __setattr__(self, key: str, value: Any) -> None:
        if key in _INTERNAL:
            object.__setattr__(self, key, value)
        else:
            self[key] = value

    def __delattr__(self, key: str) -> None:
        try:
            del self[key]
        except KeyError as e:
            raise AttributeError(key) from e

    def __deepcopy__(self, memo: dict) -> AstrBotConfig:
        clone = AstrBotConfig(self.config_path, copy.deepcopy(self.default_config, memo))
        clone.clear()
        clone.update(copy.deepcopy(dict(self), memo))
        return clone
