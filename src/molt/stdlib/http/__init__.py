"""Intrinsic-backed http package surface for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_http_client_execute", globals())

from . import client as client  # noqa: E402
from . import cookiejar as cookiejar  # noqa: E402
from . import server as server  # noqa: E402

__all__ = ["client", "cookiejar", "server"]
