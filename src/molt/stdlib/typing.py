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
    "IO",
    "Iterable",
    "Iterator",
    "Literal",
    "Optional",
    "Protocol",
    "Self",
    "TYPE_CHECKING",
    "TypeVar",
    "Union",
    "MutableMapping",
    "cast",
    "get_args",
    "get_origin",
    "overload",
    "runtime_checkable",
]

TYPE_CHECKING = False


class _TypingStub:
    __slots__ = ("__name__",)

    def __init__(self, name: str) -> None:
        self.__name__ = name

    def __repr__(self) -> str:
        return "typing." + self.__name__

    def __getitem__(self, _args: object) -> "_TypingStub":
        return self


def _make(name: str) -> _TypingStub:
    return _TypingStub(name)


Any = _make("Any")
Union = _make("Union")
Optional = _make("Optional")
Callable = _make("Callable")
ClassVar = _make("ClassVar")
Final = _make("Final")
Literal = _make("Literal")
Self = _make("Self")
Iterable = _make("Iterable")
Iterator = _make("Iterator")
MutableMapping = _make("MutableMapping")
IO = _make("IO")


def TypeVar(
    name: str,
    bound: object | None = None,
    covariant: bool = False,
    contravariant: bool = False,
) -> _TypingStub:
    _ = (bound, covariant, contravariant)
    return _TypingStub(name)


class Generic:
    @classmethod
    def __class_getitem__(cls, _params: object) -> _TypingStub:
        return _make("Generic")


class Protocol:
    @classmethod
    def __class_getitem__(cls, _params: object) -> _TypingStub:
        return _make("Protocol")


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
