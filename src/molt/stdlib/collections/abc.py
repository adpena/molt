"""Collections ABCs for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from _collections_abc import *  # noqa: F403

_require_intrinsic("molt_stdlib_probe", globals())
