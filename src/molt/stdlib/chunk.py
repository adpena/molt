"""Intrinsic-first stdlib module stub for `chunk`.

`chunk` was removed from CPython in 3.13. Molt keeps the import surface for
3.12 compatibility, but the implementation is intentionally unavailable until
there is a Rust-native lowering.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "chunk" is not fully lowered yet; only an '
        "intrinsic-first stub is available."
    )


globals().pop("_require_intrinsic", None)
