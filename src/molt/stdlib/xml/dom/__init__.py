"""Intrinsic-first stdlib package stub for `xml.dom`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P1, status:planned): replace `xml.dom` package stub with full intrinsic-backed lowering.
def __getattr__(attr: str):
    raise RuntimeError(
        'stdlib package "xml.dom" is not fully lowered yet; only an intrinsic-first stub is available.'
    )
