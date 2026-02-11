"""Minimal intrinsic-gated wsgiref package for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_WSGIREF_RUNTIME_READY = _require_intrinsic(
    "molt_wsgiref_runtime_ready", globals()
)

__all__ = ["headers", "simple_server", "util"]
