"""Minimal urllib package for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_urllib_urlsplit")

__all__ = ["error", "parse", "request"]

globals().pop("_require_intrinsic", None)
