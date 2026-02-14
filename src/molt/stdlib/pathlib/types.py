"""Intrinsic-first shim for `pathlib.types` (CPython 3.13+ layout).

This module primarily exists for typing and layout parity.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import os
from typing import TypeAlias

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_path_join", globals())

PathLike: TypeAlias = os.PathLike

__all__ = ["PathLike"]
