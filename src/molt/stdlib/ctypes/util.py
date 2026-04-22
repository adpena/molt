"""Public API surface shim for ``ctypes.util``."""

from __future__ import annotations

import shutil

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def find_library(name: str):
    if not name:
        return None
    for prefix in ("lib", ""):
        candidate = f"{prefix}{name}.dylib"
        path = shutil.which(candidate)
        if path:
            return path
    return None


def test() -> int:
    return 0


globals().pop("_require_intrinsic", None)
