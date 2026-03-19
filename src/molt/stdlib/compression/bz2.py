"""``compression.bz2`` — re-export from top-level ``bz2`` module."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from bz2 import *  # noqa: F401, F403
from bz2 import __all__ as __all__  # noqa: F811
from bz2 import BZ2Compressor, BZ2Decompressor  # noqa: F401

globals().pop("_require_intrinsic", None)
