"""Snapshot namespace keys before value hooks mutate the dictionary."""

import sys as _sys
import types as _types

first = "stored-first"
second = "stored-second"


class _Fallback(_types.ModuleType):
    def __getattribute__(self, name):
        if name == "first":
            self.__dict__["added_later"] = 100
            del self.__dict__["second"]
            return "read-first"
        return super().__getattribute__(name)

    def __getattr__(self, name):
        if name == "second":
            return "read-second"
        raise AttributeError(name)


_sys.modules[__name__].__class__ = _Fallback
