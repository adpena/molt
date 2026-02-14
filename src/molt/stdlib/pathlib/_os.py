"""Intrinsic-first shim for internal `pathlib._os` (CPython 3.13+ layout).

This module exists for CPython layout compatibility. Molt's `pathlib` calls into
Rust intrinsics for filesystem path operations.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from . import capabilities as capabilities

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_path_join", globals())

__all__ = ["capabilities"]
