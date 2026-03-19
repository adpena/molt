"""Public API surface shim for ``asyncio.log``."""

from __future__ import annotations

import logging

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

logger = logging.getLogger("asyncio")

__all__ = ["logger", "logging"]

globals().pop("_require_intrinsic", None)
