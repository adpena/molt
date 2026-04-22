"""Public API surface shim for ``ctypes.macholib.framework``."""

from __future__ import annotations

import re

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

STRICT_FRAMEWORK_RE = re.compile(
    r"(?P<location>.*/)?(?P<name>[^/]+)\.framework(?:/(?P<shortname>[^/]+))?$"
)


def framework_info(path: str):
    match = STRICT_FRAMEWORK_RE.match(path)
    if match is None:
        return None
    return match.groupdict()


globals().pop("_require_intrinsic", None)
