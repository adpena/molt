"""Compression package — unified namespace for compression modules (Python 3.14+).

Re-exports the standalone compression modules: bz2, gzip, lzma, zlib.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

__all__ = ["bz2", "gzip", "lzma", "zlib", "zstd"]

globals().pop("_require_intrinsic", None)
