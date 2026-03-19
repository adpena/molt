"""Version-gated `asyncio.graph` import behavior."""

import sys

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


def _raise_missing():
    raise ModuleNotFoundError("No module named 'asyncio.graph'")


if getattr(sys, "version_info", (0, 0))[1] < 14:
    _raise_missing()
