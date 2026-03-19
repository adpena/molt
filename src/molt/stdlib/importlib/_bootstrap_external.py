"""Compatibility surface for CPython ``importlib._bootstrap_external``."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from _frozen_importlib_external import *  # noqa: F401,F403
from _frozen_importlib_external import __all__ as _FROZEN_EXTERNAL_ALL

__all__ = list(_FROZEN_EXTERNAL_ALL)

globals().pop("_require_intrinsic", None)
