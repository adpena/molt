"""Compatibility surface for CPython ``importlib._bootstrap_external``."""


from _frozen_importlib_external import *  # noqa: F401,F403
from _frozen_importlib_external import __all__ as _FROZEN_EXTERNAL_ALL

__all__ = list(_FROZEN_EXTERNAL_ALL)
