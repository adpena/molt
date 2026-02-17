"""Intrinsic-first minimal `array` surface for typed short arrays."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

__all__ = ["array", "typecodes"]

# Keep the advertised surface explicit while the module remains intrinsic-partial.
typecodes = "h"


def _pack_signed_short(initializer) -> bytearray:
    out = bytearray()
    for item in initializer:
        value = int(item)
        if value < -32768 or value > 32767:
            raise OverflowError("signed short integer is out of range")
        # Molt currently targets little-endian native runtimes.
        out.extend(value.to_bytes(2, byteorder="little", signed=True))
    return out


def array(typecode: str, initializer=()):
    if not isinstance(typecode, str) or len(typecode) != 1:
        raise TypeError("array() argument 1 must be a unicode character, not str")
    if typecode != "h":
        raise RuntimeError(
            f'array typecode "{typecode}" is not yet lowered into Molt intrinsics'
        )
    packed = _pack_signed_short(initializer)
    # Runtime-backed typed buffer view. `memoryview(array(...))` preserves format `h`.
    return memoryview(packed).cast("h")
