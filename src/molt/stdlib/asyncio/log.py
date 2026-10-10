"""Public API surface shim for ``asyncio.log``."""

from __future__ import annotations

import logging


logger = logging.getLogger("asyncio")

__all__ = ["logger", "logging"]
