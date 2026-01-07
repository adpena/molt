"""Minimal typing shim for Molt.

This shim keeps annotation spelling working without importing CPython's
full typing module. It is intentionally small and deterministic.
"""

from __future__ import annotations

__all__ = [
    "Any",
    "Callable",
    "ClassVar",
    "Final",
    "Generic",
    "Literal",
    "Optional",
    "Protocol",
    "Self",
    "TYPE_CHECKING",
    "TypeVar",
    "Union",
    "cast",
    "get_args",
    "get_origin",
    "overload",
    "runtime_checkable",
]

TYPE_CHECKING = False


class _TypingForm:
    __slots__ = ("__name__", "__args__", "__origin__")

    def __init__(
        self, name: str, args: tuple[object, ...] | None = None, origin=None
    ) -> None:
        self.__name__ = name
        self.__args__ = args or ()
        self.__origin__ = origin if origin is not None else self

    def __repr__(self) -> str:
        if not self.__args__:
            return f"typing.{self.__name__}"
        args = ", ".join(repr(arg) for arg in self.__args__)
        return f"typing.{self.__name__}[{args}]"

    def __getitem__(self, args: object) -> "_TypingForm":
        if not isinstance(args, tuple):
            args = (args,)
        return _TypingForm(self.__name__, args, origin=self)


class TypeVar:
    def __init__(
        self,
        name: str,
        *constraints: object,
        bound: object | None = None,
        covariant: bool = False,
        contravariant: bool = False,
    ) -> None:
        self.__name__ = name
        self.__constraints__ = constraints
        self.__bound__ = bound
        self.__covariant__ = covariant
        self.__contravariant__ = contravariant

    def __repr__(self) -> str:
        return f"~{self.__name__}"


class Generic:
    @classmethod
    def __class_getitem__(cls, params: object) -> _TypingForm:
        if not isinstance(params, tuple):
            params = (params,)
        return _TypingForm(cls.__name__, params)


class Protocol:
    @classmethod
    def __class_getitem__(cls, params: object) -> _TypingForm:
        if not isinstance(params, tuple):
            params = (params,)
        return _TypingForm("Protocol", params)


Any = _TypingForm("Any")
Union = _TypingForm("Union")
Optional = _TypingForm("Optional")
Callable = _TypingForm("Callable")
ClassVar = _TypingForm("ClassVar")
Final = _TypingForm("Final")
Literal = _TypingForm("Literal")
Self = _TypingForm("Self")


def cast(_typ: object, value: object) -> object:
    return value


def get_origin(tp: object) -> object | None:
    return getattr(tp, "__origin__", None)


def get_args(tp: object) -> tuple[object, ...]:
    return getattr(tp, "__args__", ())


def overload(func):
    return func


def runtime_checkable(cls):
    return cls


# TODO(stdlib-compat, owner:stdlib, milestone:SL3): expand typing helpers + Protocol semantics.
