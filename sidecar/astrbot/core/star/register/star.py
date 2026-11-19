from __future__ import annotations

from astrbot.core.star.star import StarMetadata, star_map, star_registry


def register_star(name: str, author: str, desc: str, version: str, repo: str | None = None):
    """Attaches name, author, description, version and repo to the plugin class."""

    def decorator(cls):
        md = star_map.get(cls.__module__)
        if md is None:
            md = StarMetadata(module_path=cls.__module__)
            star_map[cls.__module__] = md
            star_registry.append(md)
        md.name, md.author, md.desc, md.version, md.repo = name, author, desc, version, repo
        md.star_cls_type = cls
        md.registered = True
        return cls

    return decorator
