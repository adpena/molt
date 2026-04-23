"""Intrinsic-first stdlib module stub for `numbers`.

The numeric ABC tower affects `isinstance`/`issubclass` semantics and must be
runtime-owned. Molt raises here until the ABC hierarchy is lowered natively.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "numbers" is not fully lowered yet; only an '
        "intrinsic-first stub is available."
    )


globals().pop("_require_intrinsic", None)
