from __future__ import annotations

import enum
from typing import Any

from astrbot.core.star.filter import HandlerFilter


class PermissionType(enum.Flag):
    ADMIN = enum.auto()
    MEMBER = enum.auto()
    GROUP_ADMIN = enum.auto()
    SHARED_GROUP_ADMIN = enum.auto()


class PermissionTypeFilter(HandlerFilter):
    """ADMIN requires a bot admin. GROUP_ADMIN variants require it in groups only."""

    def __init__(self, permission_type: PermissionType, raise_error: bool = True) -> None:
        self.permission_type = permission_type
        self.raise_error = raise_error

    def filter(self, event: Any, cfg: Any) -> bool:
        if self.permission_type == PermissionType.ADMIN:
            return event.is_admin()
        if self.permission_type in (PermissionType.GROUP_ADMIN, PermissionType.SHARED_GROUP_ADMIN):
            return event.is_admin() if event.get_group_id() else True
        return True
