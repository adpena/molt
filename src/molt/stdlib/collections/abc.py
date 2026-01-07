"""Import-only collections.abc stubs for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement real ABC checks/registration.

__all__ = [
    "AsyncGenerator",
    "AsyncIterable",
    "AsyncIterator",
    "Awaitable",
    "Callable",
    "Collection",
    "Container",
    "Coroutine",
    "Generator",
    "Hashable",
    "Iterable",
    "Iterator",
    "Mapping",
    "MutableMapping",
    "MutableSequence",
    "MutableSet",
    "Reversible",
    "Sequence",
    "Set",
    "Sized",
]


class _ABCBase:
    __slots__ = ()

    def __class_getitem__(cls, _item):
        return cls


class Hashable(_ABCBase):
    pass


class Iterable(_ABCBase):
    pass


class Iterator(Iterable):
    pass


class Reversible(Iterable):
    pass


class Sized(_ABCBase):
    pass


class Container(_ABCBase):
    pass


class Collection(Iterable):
    pass


class Callable(_ABCBase):
    pass


class Sequence(Collection):
    pass


class MutableSequence(Sequence):
    pass


class Set(Collection):
    pass


class MutableSet(Set):
    pass


class Mapping(Collection):
    pass


class MutableMapping(Mapping):
    pass


class Awaitable(_ABCBase):
    pass


class Coroutine(Awaitable):
    pass


class AsyncIterable(_ABCBase):
    pass


class AsyncIterator(AsyncIterable):
    pass


class Generator(Iterator):
    pass


class AsyncGenerator(AsyncIterator):
    pass
