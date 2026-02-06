"""Errno constants for Molt.

In compiled code, errno constants come from the runtime via `_molt_errno_constants()`.
Missing intrinsics are a hard error (no host fallback).
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


def _load_errno_constants() -> tuple[dict[str, int], dict[int, str]]:
    intrinsic = _require_intrinsic("molt_errno_constants", globals())
    try:
        res = intrinsic()
    except Exception:
        res = None
    if isinstance(res, tuple) and len(res) == 2:
        left, right = res
        if isinstance(left, dict) and isinstance(right, dict):
            return left, right
    raise RuntimeError("errno intrinsics unavailable")


constants, errorcode = _load_errno_constants()
globals().update(constants)
__all__ = sorted(constants) + ["errorcode"]
