"""Subset of importlib.abc used to reduce importlib.util imports."""

import abc

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_LOAD_MODULE_SHIM = _require_intrinsic(
    "molt_importlib_load_module_shim"
)
from . import _bootstrap


class Loader(metaclass=abc.ABCMeta):
    """Abstract base class for import loaders."""

    def create_module(self, spec):
        return None

    def load_module(self, fullname):
        if not hasattr(self, "exec_module"):
            raise ImportError
        return _MOLT_IMPORTLIB_LOAD_MODULE_SHIM(_bootstrap, self, fullname)
