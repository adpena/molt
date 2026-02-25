"""``compression.gzip`` — re-export from top-level ``gzip`` module."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

from gzip import *  # noqa: F401, F403
from gzip import __all__ as __all__  # noqa: F811
from gzip import BadGzipFile, GzipFile  # noqa: F401
