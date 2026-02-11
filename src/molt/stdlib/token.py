"""Minimal `token` constants for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

ENDMARKER = 0
NAME = 1
NUMBER = 2
NEWLINE = 4
INDENT = 5
DEDENT = 6
OP = 54
COMMENT = 64
NL = 65
ENCODING = 67
