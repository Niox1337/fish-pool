"""Compatibility package that runs AstrBot plugins inside the fish sidecar.

Only the public plugin API is provided. The list of supported members is kept
in COMPAT.md next to this package.
"""

import logging

from astrbot._unsupported import UnsupportedFeature, install_finder

__version__ = "4.24.0"

logger = logging.getLogger("astrbot")

install_finder()

__all__ = ["UnsupportedFeature", "logger"]
