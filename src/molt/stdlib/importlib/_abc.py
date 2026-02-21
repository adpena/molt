"""Subset of importlib.abc used to reduce importlib.util imports."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import abc

from . import _bootstrap


class Loader(metaclass=abc.ABCMeta):
    """Abstract base class for import loaders."""

    def create_module(self, spec):
        return None

    def load_module(self, fullname):
        if not hasattr(self, "exec_module"):
            raise ImportError
        return _bootstrap._load_module_shim(self, fullname)
