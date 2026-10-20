"""Token constants for the compiled Python target."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


globals().update(_require_intrinsic("molt_token_payload")())

# Bind the names used by the Python helpers from the same runtime payload.
NT_OFFSET = globals()["NT_OFFSET"]
ENDMARKER = globals()["ENDMARKER"]


def ISTERMINAL(x: int) -> bool:
    return x < NT_OFFSET


def ISNONTERMINAL(x: int) -> bool:
    return x >= NT_OFFSET


def ISEOF(x: int) -> bool:
    return x == ENDMARKER


globals().pop("_require_intrinsic", None)
