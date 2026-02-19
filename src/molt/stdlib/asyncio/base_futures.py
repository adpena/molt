"""Public API surface shim for ``asyncio.base_futures``."""

from __future__ import annotations

import reprlib

from _intrinsics import require_intrinsic as _require_intrinsic

from . import format_helpers

_require_intrinsic("molt_capabilities_has", globals())


def isfuture(obj) -> bool:
    cls = obj.__class__
    if cls.__name__ == "Future":
        return True
    return getattr(obj, "_asyncio_future_blocking", None) is not None


__all__ = ["format_helpers", "isfuture", "reprlib"]
