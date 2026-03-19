"""Public API surface shim for ``ctypes.macholib.dylib``."""

from __future__ import annotations

import re

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

DYLIB_RE = re.compile(
    r"(?P<name>.+)\.dylib(?:\.(?P<version>[^_]+))?(?:_(?P<suffix>.+))?$"
)


def dylib_info(path: str):
    match = DYLIB_RE.match(path)
    if match is None:
        return None
    return match.groupdict()

globals().pop("_require_intrinsic", None)
