"""Compatibility surface for CPython ``importlib._bootstrap``."""


from _frozen_importlib import *  # noqa: F401,F403
from _frozen_importlib import __all__ as _FROZEN_ALL
from _frozen_importlib import _DeadlockError, _find_spec

__all__ = list(_FROZEN_ALL)
