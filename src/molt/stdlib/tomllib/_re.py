"""Intrinsic-first stdlib module stub for `tomllib._re`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `tomllib._re` module stub with full intrinsic-backed lowering.
def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "tomllib._re" is not fully lowered yet; only an intrinsic-first stub is available.'
    )
