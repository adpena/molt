"""Collections ABCs for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from _collections_abc import *  # noqa: F403

_MOLT_ABC_BOOTSTRAP = _require_intrinsic("molt_abc_bootstrap", globals())
