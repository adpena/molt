"""``compression.lzma`` — re-export from top-level ``lzma`` module."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from lzma import *  # noqa: F401, F403
from lzma import __all__ as __all__  # noqa: F811
from lzma import LZMACompressor, LZMADecompressor, LZMAError  # noqa: F401

globals().pop("_require_intrinsic", None)
