"""Public API surface shim for ``asyncio.log``."""

from __future__ import annotations

import logging

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

logger = logging.getLogger("asyncio")

__all__ = ["logger", "logging"]
