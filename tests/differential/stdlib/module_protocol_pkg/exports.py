"""Resolve both __all__ and each selected value through module hooks."""

import sys as _sys
import types as _types


class _Exports(_types.ModuleType):
    def __getattr__(self, name):
        if name == "__all__":
            return ["exported"]
        if name == "exported":
            return 91
        raise AttributeError(name)


_sys.modules[__name__].__class__ = _Exports
