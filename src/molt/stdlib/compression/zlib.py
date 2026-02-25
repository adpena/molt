"""``compression.zlib`` — re-export from top-level ``zlib`` module."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

from zlib import *  # noqa: F401, F403
from zlib import __all__ as __all__  # noqa: F811
from zlib import error  # noqa: F401
