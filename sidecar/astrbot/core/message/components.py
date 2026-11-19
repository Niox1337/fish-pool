"""Message components. Constructors accept the keyword forms AstrBot plugins use."""

from __future__ import annotations

import asyncio
import base64
import os
import urllib.parse
import urllib.request
import uuid
from enum import Enum
from pathlib import Path
from typing import Any

from astrbot._unsupported import unsupported


class ComponentType(str, Enum):
    Plain = "Plain"
    Image = "Image"
    Record = "Record"
    Video = "Video"
    File = "File"
    Face = "Face"
    At = "At"
    Node = "Node"
    Nodes = "Nodes"
    Poke = "Poke"
    Reply = "Reply"
    Forward = "Forward"
    RPS = "RPS"
    Dice = "Dice"
    Shake = "Shake"
    Share = "Share"
    Contact = "Contact"
    Location = "Location"
    Music = "Music"
    Json = "Json"
    Unknown = "Unknown"


class BaseMessageComponent:
    type: ComponentType = ComponentType.Unknown

    def __init__(self, **kwargs: Any) -> None:
        for key, value in kwargs.items():
            setattr(self, key, value)

    def _fields(self) -> dict[str, Any]:
        return {k: v for k, v in vars(self).items() if not k.startswith("_")}

    def toDict(self) -> dict:
        data = {k: v for k, v in self._fields().items() if v is not None}
        return {"type": self.type.value.lower(), "data": data}

    async def to_dict(self) -> dict:
        return self.toDict()

    def model_dump(self) -> dict:
        return {"type": self.type.value, **self._fields()}

    def __eq__(self, other: object) -> bool:
        return type(self) is type(other) and self._fields() == other._fields()  # type: ignore[union-attr]

    def __repr__(self) -> str:
        args = ", ".join(f"{k}={v!r}"[:120] for k, v in self._fields().items())
        return f"{type(self).__name__}({args})"


class Plain(BaseMessageComponent):
    type = ComponentType.Plain
    text: str = ""

    def __init__(self, text: str = "", convert: bool = True, **kwargs: Any) -> None:
        super().__init__(text=text, **kwargs)
        self.convert = convert

    def toDict(self) -> dict:
        return {"type": "text", "data": {"text": self.text.strip()}}


def _temp_dir() -> Path:
    from astrbot.core.utils.astrbot_path import get_astrbot_temp_path

    return Path(get_astrbot_temp_path())


def _download(url: str, suffix: str) -> str:
    target = _temp_dir() / f"dl_{uuid.uuid4().hex}{suffix}"
    request = urllib.request.Request(url, headers={"User-Agent": "fish-sidecar"})
    with urllib.request.urlopen(request, timeout=30) as response, open(target, "wb") as out:
        while chunk := response.read(65536):
            out.write(chunk)
    return str(target.resolve())


def _file_uri_to_path(uri: str) -> str:
    parsed = urllib.parse.urlparse(uri)
    return urllib.request.url2pathname(parsed.path)


async def _resolve_to_path(ref: str, suffix: str) -> str:
    """Turns a URL, file URI, base64 reference or local path into a local file path."""
    if not ref:
        raise ValueError("No valid file or URL provided")
    if ref.startswith("file://"):
        path = _file_uri_to_path(ref)
    elif ref.startswith("base64://"):
        target = _temp_dir() / f"b64_{uuid.uuid4().hex}{suffix}"
        target.write_bytes(base64.b64decode(ref[len("base64://") :]))
        return str(target.resolve())
    elif ref.startswith(("http://", "https://")):
        return await asyncio.to_thread(_download, ref, suffix)
    else:
        path = ref
    if not os.path.exists(path):
        raise FileNotFoundError(path)
    if not os.path.isfile(path):
        raise ValueError(f"not a regular file: {path}")
    return os.path.abspath(path)


async def _resolve_to_base64(ref: str, suffix: str) -> str:
    if ref.startswith("base64://"):
        return ref[len("base64://") :]
    path = await _resolve_to_path(ref, suffix)
    return base64.b64encode(Path(path).read_bytes()).decode()


class _Media(BaseMessageComponent):
    file: str | None = ""
    url: str | None = ""
    path: str | None = ""
    _suffix = ""

    def __init__(self, file: str | None = "", **kwargs: Any) -> None:
        kwargs.setdefault("url", "")
        kwargs.setdefault("path", "")
        super().__init__(file=file, **kwargs)

    @classmethod
    def fromURL(cls, url: str, **kwargs: Any):
        if url.startswith(("http://", "https://")):
            return cls(file=url, url=url, **kwargs)
        raise ValueError("not a valid url")

    @classmethod
    def fromFileSystem(cls, path: str | os.PathLike, **kwargs: Any):
        resolved = Path(path).resolve(strict=False)
        return cls(file=resolved.as_uri(), path=str(resolved), **kwargs)

    @classmethod
    def fromBase64(cls, data: str, **kwargs: Any):
        return cls(file=f"base64://{data}", **kwargs)

    @classmethod
    def fromBytes(cls, data: bytes):
        return cls.fromBase64(base64.b64encode(data).decode())

    @classmethod
    def fromIO(cls, io: Any):
        return cls.fromBytes(io.read())

    def _ref(self) -> str:
        return self.url or self.file or self.path or ""

    async def convert_to_file_path(self) -> str:
        return await _resolve_to_path(self._ref(), self._suffix)

    async def convert_to_base64(self) -> str:
        return await _resolve_to_base64(self._ref(), self._suffix)

    async def register_to_file_service(self) -> str:
        unsupported(f"{type(self).__name__}.register_to_file_service")


class Image(_Media):
    type = ComponentType.Image
    _suffix = ".jpg"


class Record(_Media):
    type = ComponentType.Record
    text: str | None = None
    _suffix = ".amr"


class Video(_Media):
    type = ComponentType.Video
    cover: str | None = ""
    _suffix = ".mp4"


class File(BaseMessageComponent):
    type = ComponentType.File
    name: str | None = ""
    url: str | None = ""

    def __init__(self, name: str = "", file: str = "", url: str = "", **kwargs: Any) -> None:
        super().__init__(name=name, url=url, **kwargs)
        self.file_ = file

    @property
    def file(self) -> str:
        return self.file_

    @file.setter
    def file(self, value: str) -> None:
        if value.startswith(("http://", "https://")):
            self.url = value
        else:
            self.file_ = value

    async def get_file(self, allow_return_url: bool = False) -> str:
        if allow_return_url and self.url:
            return self.url
        if self.file_:
            return await _resolve_to_path(self.file_, "")
        if self.url:
            return await _resolve_to_path(self.url, Path(self.name or "").suffix)
        return ""


class Face(BaseMessageComponent):
    type = ComponentType.Face
    id: int = 0


class At(BaseMessageComponent):
    type = ComponentType.At
    qq: int | str = ""
    name: str | None = ""

    def toDict(self) -> dict:
        return {"type": "at", "data": {"qq": str(self.qq)}}


class AtAll(At):
    def __init__(self, **kwargs: Any) -> None:
        kwargs["qq"] = "all"
        super().__init__(**kwargs)


class Reply(BaseMessageComponent):
    type = ComponentType.Reply
    id: str | int = ""
    chain: list[BaseMessageComponent] | None = None
    sender_id: int | str | None = 0
    sender_nickname: str | None = ""
    time: int | None = 0
    message_str: str | None = ""
    text: str | None = ""
    qq: int | None = 0
    seq: int | None = 0

    def __init__(self, **kwargs: Any) -> None:
        kwargs.setdefault("chain", [])
        super().__init__(**kwargs)


class Poke(BaseMessageComponent):
    type = ComponentType.Poke
    id: int | str | None = 0
    qq: int | str | None = 0

    def __init__(self, poke_type: str | int | None = None, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self._type = poke_type or "126"


class Forward(BaseMessageComponent):
    type = ComponentType.Forward
    id: str = ""


class Node(BaseMessageComponent):
    type = ComponentType.Node
    id: int | None = 0
    name: str | None = ""
    uin: str | None = "0"
    seq: str | list | None = ""
    time: int | None = 0

    def __init__(self, content: list[BaseMessageComponent] | None = None, **kwargs: Any) -> None:
        super().__init__(content=list(content or []), **kwargs)


class Nodes(BaseMessageComponent):
    type = ComponentType.Nodes

    def __init__(self, nodes: list[Node] | None = None, **kwargs: Any) -> None:
        super().__init__(nodes=list(nodes or []), **kwargs)


class Json(BaseMessageComponent):
    type = ComponentType.Json

    def __init__(self, data: str | dict | None = None, **kwargs: Any) -> None:
        if isinstance(data, str):
            import json

            data = json.loads(data)
        super().__init__(data=data or {}, **kwargs)


class Share(BaseMessageComponent):
    type = ComponentType.Share
    url: str = ""
    title: str = ""
    content: str | None = ""
    image: str | None = ""


class Music(BaseMessageComponent):
    type = ComponentType.Music
    id: int | None = 0
    url: str | None = ""
    audio: str | None = ""
    title: str | None = ""
    content: str | None = ""
    image: str | None = ""


class Location(BaseMessageComponent):
    type = ComponentType.Location
    lat: float = 0.0
    lon: float = 0.0
    title: str | None = ""
    content: str | None = ""


class Contact(BaseMessageComponent):
    type = ComponentType.Contact
    id: int | None = 0


class RPS(BaseMessageComponent):
    type = ComponentType.RPS


class Dice(BaseMessageComponent):
    type = ComponentType.Dice


class Shake(BaseMessageComponent):
    type = ComponentType.Shake


class Unknown(BaseMessageComponent):
    type = ComponentType.Unknown
    text: str = ""


ComponentTypes = {
    "plain": Plain,
    "text": Plain,
    "image": Image,
    "record": Record,
    "video": Video,
    "file": File,
    "face": Face,
    "at": At,
    "reply": Reply,
    "poke": Poke,
    "forward": Forward,
    "node": Node,
    "nodes": Nodes,
    "json": Json,
    "share": Share,
    "music": Music,
    "location": Location,
    "contact": Contact,
    "rps": RPS,
    "dice": Dice,
    "shake": Shake,
    "unknown": Unknown,
}

__all__ = [
    "BaseMessageComponent",
    "ComponentType",
    "ComponentTypes",
    "Plain",
    "Image",
    "Record",
    "Video",
    "File",
    "Face",
    "At",
    "AtAll",
    "Reply",
    "Poke",
    "Forward",
    "Node",
    "Nodes",
    "Json",
    "Share",
    "Music",
    "Location",
    "Contact",
    "RPS",
    "Dice",
    "Shake",
    "Unknown",
]
