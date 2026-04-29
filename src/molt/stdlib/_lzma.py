"""Low-level lzma helpers used by `lzma`.

CPython exposes this as a C extension module that the public `lzma`
Python module imports `LZMACompressor` and `LZMADecompressor` from. Molt's
`lzma` module already implements both classes against the runtime
intrinsics (`molt_lzma_compressor_*`, `molt_lzma_decompressor_*`), so
`_lzma` simply re-exports them so any third-party code that imports `_lzma`
directly gets the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


from lzma import (
    CHECK_CRC32,
    CHECK_CRC64,
    CHECK_NONE,
    CHECK_SHA256,
    FORMAT_ALONE,
    FORMAT_AUTO,
    FORMAT_RAW,
    FORMAT_XZ,
    LZMACompressor,
    LZMADecompressor,
)


__all__ = [
    "LZMACompressor",
    "LZMADecompressor",
    "FORMAT_AUTO",
    "FORMAT_XZ",
    "FORMAT_ALONE",
    "FORMAT_RAW",
    "CHECK_NONE",
    "CHECK_CRC32",
    "CHECK_CRC64",
    "CHECK_SHA256",
]


globals().pop("_require_intrinsic", None)
