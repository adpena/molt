"""Minimal abc support for Molt."""

from __future__ import annotations

__all__ = ["ABC", "ABCMeta", "abstractmethod", "get_cache_token"]


def abstractmethod(func):
    setattr(func, "__isabstractmethod__", True)
    return func


def _is_abstract(obj: object) -> bool:
    if getattr(obj, "__isabstractmethod__", False):
        return True
    fget = getattr(obj, "fget", None)
    fset = getattr(obj, "fset", None)
    fdel = getattr(obj, "fdel", None)
    if fget is not None or fset is not None or fdel is not None:
        for attr in (fget, fset, fdel):
            if getattr(attr, "__isabstractmethod__", False):
                return True
    return False


class ABCMeta(type):
    def __new__(mcls, name, bases, namespace, **kwargs):
        cls = super().__new__(mcls, name, bases, dict(namespace))
        abstracts: set[str] = set()
        for base in bases:
            abstracts.update(getattr(base, "__abstractmethods__", set()))
        for attr_name, attr_value in namespace.items():
            if _is_abstract(attr_value):
                abstracts.add(attr_name)
            else:
                abstracts.discard(attr_name)
        cls.__abstractmethods__ = frozenset(abstracts)
        return cls

    def __call__(cls, *args, **kwargs):
        if getattr(cls, "__abstractmethods__", None):
            missing = ", ".join(sorted(cls.__abstractmethods__))
            raise TypeError(
                f"Can't instantiate abstract class {cls.__name__} with abstract methods {missing}"
            )
        return super().__call__(*args, **kwargs)


class ABC(metaclass=ABCMeta):
    pass


_CACHE_TOKEN = 0


def get_cache_token() -> int:
    global _CACHE_TOKEN
    _CACHE_TOKEN += 1
    return _CACHE_TOKEN
