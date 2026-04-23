"""Purpose: ABCMeta-style __new__ calls super then _abc_init."""

from _abc import _abc_init


class ABCMeta(type):
    def __new__(mcls, name, bases, namespace, /, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace, **kwargs)
        _abc_init(cls)
        return cls


class ABC(metaclass=ABCMeta):
    __slots__ = ()


print(ABC.__name__)
