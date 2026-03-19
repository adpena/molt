"""Compatibility surface for CPython ``importlib._bootstrap``."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from _frozen_importlib import *  # noqa: F401,F403
from _frozen_importlib import __all__ as _FROZEN_ALL

__all__ = list(_FROZEN_ALL)
