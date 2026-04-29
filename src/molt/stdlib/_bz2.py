"""Low-level bz2 helpers used by `bz2`.

CPython exposes this as a C extension module from which the public `bz2`
Python module imports `BZ2Compressor` and `BZ2Decompressor`. Molt's `bz2`
module already implements both classes against the Rust runtime intrinsics
(`molt_bz2_compressor_*`, `molt_bz2_decompressor_*`), so `_bz2` simply
re-exports them so any third-party code that imports `_bz2` directly gets
the working implementation.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


from bz2 import BZ2Compressor, BZ2Decompressor


__all__ = ["BZ2Compressor", "BZ2Decompressor"]


globals().pop("_require_intrinsic", None)
