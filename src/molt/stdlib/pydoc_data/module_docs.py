"""Intrinsic-first stdlib module stub for `pydoc_data.module_docs`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# STDLIB_GAP(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `pydoc_data.module_docs` module stub with full intrinsic-backed lowering.
def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib module "pydoc_data.module_docs" is not fully lowered yet; only an intrinsic-first stub is available.'
    )
