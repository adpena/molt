"""Minimal intrinsic-gated `xmlrpc` package for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_XMLRPC_RUNTIME_READY = _require_intrinsic("molt_xmlrpc_runtime_ready", globals())

__all__ = ["client", "server"]
