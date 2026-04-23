"""Intrinsic-first stdlib module stub for `unittest._log`."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "unittest._log" is not fully lowered yet; only an '
        "intrinsic-first stub is available."
    )


globals().pop("_require_intrinsic", None)
