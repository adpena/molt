"""Intrinsic-first shim for internal `pathlib._abc` (CPython 3.13+ layout).

Molt's `pathlib` is a package for 3.12/3.13/3.14 union coverage. This module
exists for layout parity and to keep imports deterministic.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from . import Path as Path  # re-export

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_path_join", globals())

__all__ = ["Path"]
