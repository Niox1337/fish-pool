"""Conversion between message components and the fish node JSON component format."""

from __future__ import annotations

import logging
from typing import Any

from astrbot.core.message import components as C
from astrbot.core.message.message_event_result import MessageChain
from astrbot.core.utils.media_utils import file_uri_to_path

log = logging.getLogger("astrbot")


def _media(comp: C.Image | C.Record, kind: str) -> dict[str, Any]:
    out: dict[str, Any] = {"type": kind}
    ref = comp.file or ""
    url = comp.url or ""
    if url.startswith(("http://", "https://")):
        out["url"] = url
    elif ref.startswith(("http://", "https://")):
        out["url"] = ref
    elif ref.startswith("base64://"):
        out["base64"] = ref[len("base64://") :]
    elif comp.path:
        out["path"] = comp.path
    elif ref.startswith("file://"):
        out["path"] = file_uri_to_path(ref)
    elif ref:
        out["path"] = ref
    if kind == "record":
        out.pop("base64", None)
    return out


def component_to_wire(comp: Any) -> dict[str, Any] | None:
    if isinstance(comp, str):
        return {"type": "plain", "text": comp}
    if isinstance(comp, C.Plain):
        return {"type": "plain", "text": comp.text}
    if isinstance(comp, C.Image):
        return _media(comp, "image")
    if isinstance(comp, C.Record):
        return _media(comp, "record")
    if isinstance(comp, C.At):
        out: dict[str, Any] = {"type": "at", "user_id": str(comp.qq)}
        if comp.name:
            out["name"] = comp.name
        return out
    if isinstance(comp, C.Reply):
        return {"type": "reply", "id": str(comp.id)}
    if isinstance(comp, C.Face):
        return {"type": "face", "id": comp.id}
    log.warning("component %s cannot be sent by the fish node and was dropped", type(comp).__name__)
    return None


def chain_to_wire(chain: Any) -> list[dict[str, Any]]:
    if chain is None:
        return []
    if isinstance(chain, MessageChain):
        items = chain.chain
    elif isinstance(chain, str):
        items = [chain]
    else:
        items = list(chain)
    return [d for d in (component_to_wire(c) for c in items) if d is not None]


def component_from_wire(data: dict[str, Any]) -> C.BaseMessageComponent:
    kind = data.get("type")
    if kind == "plain":
        return C.Plain(data.get("text", ""))
    if kind in ("image", "record"):
        cls = C.Image if kind == "image" else C.Record
        if data.get("url"):
            return cls(file=data["url"], url=data["url"])
        if data.get("path"):
            return cls.fromFileSystem(data["path"])
        if data.get("base64"):
            return cls.fromBase64(data["base64"])
        return cls(file="")
    if kind == "at":
        return C.At(qq=data.get("user_id", ""), name=data.get("name") or "")
    if kind == "reply":
        return C.Reply(id=data.get("id", ""), chain=[])
    if kind == "face":
        try:
            return C.Face(id=int(data.get("id", 0)))
        except (TypeError, ValueError):
            return C.Face(id=data.get("id"))
    return C.Unknown(text=str(data))


def chain_from_wire(items: list[dict[str, Any]] | None) -> list[C.BaseMessageComponent]:
    return [component_from_wire(d) for d in items or [] if isinstance(d, dict)]
