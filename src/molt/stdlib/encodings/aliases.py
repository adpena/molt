"""Encoding alias registry backed by runtime intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_ENCODINGS_ALIASES_MAP = _require_intrinsic(
    "molt_encodings_aliases_map", globals()
)

_aliases_obj = _MOLT_ENCODINGS_ALIASES_MAP()
if not isinstance(_aliases_obj, dict):
    raise RuntimeError("invalid encodings.aliases intrinsic payload")

aliases = _aliases_obj
