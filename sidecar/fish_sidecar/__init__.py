"""Plugin host for fish: a fork server and the per plugin process runtime.

Importing the package prepares the AstrBot import (``astrbot_boot.prepare_import``),
so every module here that imports AstrBot gets the patched thread owners and a
scratch ``ASTRBOT_ROOT`` instead of the working directory.
"""

from fish_sidecar import astrbot_boot as _astrbot_boot

_astrbot_boot.prepare_import()

__version__ = "0.1.0"
