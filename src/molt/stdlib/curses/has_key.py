"""Public API surface shim for ``curses.has_key``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")


def has_key(_ch: int) -> bool:
    return False

globals().pop("_require_intrinsic", None)
