"""Intrinsic-first stdlib module stub for `dbm.sqlite3`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `dbm.sqlite3` module stub with full intrinsic-backed lowering.
def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "dbm.sqlite3" is not fully lowered yet; only an intrinsic-first stub is available.'
    )
