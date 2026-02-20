"""Windows-specific spawn backend (CPython-compatible import failure on non-Windows)."""

from _intrinsics import require_intrinsic as _require_intrinsic
import os as _os

_require_intrinsic("molt_capabilities_has", globals())

if _os.name != "nt":
    raise ModuleNotFoundError("No module named 'msvcrt'")

import msvcrt  # noqa: F401
