"""Minimal urllib package for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_urllib_urlsplit", globals())

__all__ = ["error", "parse", "request"]
