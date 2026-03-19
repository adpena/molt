"""Molt-backed `_opcode` module."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready"
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

# Keep `_opcode` public callables as intrinsic-backed builtins so API-shape
# probes see CPython-like builtin function objects.
get_specialization_stats = _require_intrinsic(
    "molt_opcode_get_specialization_stats"
)
stack_effect = _require_intrinsic("molt_opcode_stack_effect")


globals().pop("_require_intrinsic", None)
