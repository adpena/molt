"""Collections helpers for Molt."""

from __future__ import annotations

from typing import Any, Iterable, cast

import collections.abc as abc

__all__ = ["abc", "Counter", "defaultdict", "deque"]


class _DequeIter:
    def __init__(self, deq: "deque") -> None:
        self._data = deq._data
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self) -> Any:
        if self._index >= len(self._data):
            raise StopIteration
        value = self._data[self._index]
        self._index += 1
        return value


class deque:
    _iter_class = _DequeIter

    def __init__(
        self, iterable: Iterable[Any] | None = None, maxlen: int | None = None
    ):
        if maxlen is not None and maxlen < 0:
            raise ValueError("maxlen must be non-negative")
        self._maxlen = maxlen
        self._data: list[Any] = []
        if iterable is not None:
            for item in iterable:
                self.append(item)

    def __len__(self) -> int:
        return len(self._data)

    def append(self, item: Any) -> None:
        if self._maxlen is not None and len(self._data) == self._maxlen:
            self._data.pop(0)
        self._data.append(item)

    def appendleft(self, item: Any) -> None:
        if self._maxlen is not None and len(self._data) == self._maxlen:
            self._data.pop()
        self._data.insert(0, item)

    def pop(self) -> Any:
        if not self._data:
            raise IndexError("pop from an empty deque")
        return self._data.pop()

    def popleft(self) -> Any:
        if not self._data:
            raise IndexError("pop from an empty deque")
        return self._data.pop(0)

    def clear(self) -> None:
        self._data.clear()

    def extend(self, iterable: Iterable[Any]) -> None:
        for item in iterable:
            self.append(item)

    def extendleft(self, iterable: Iterable[Any]) -> None:
        for item in iterable:
            self.appendleft(item)

    def __iter__(self):
        return self._iter_class(self)

    def __repr__(self) -> str:
        return f"deque({list(self)!r})"


class Counter:
    def __init__(
        self,
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None,
        **kwargs: Any,
    ) -> None:
        self._data: dict = {}
        if iterable is not None:
            if isinstance(iterable, dict) or hasattr(iterable, "items"):
                mapping = cast(Any, iterable)
                for key, val in mapping.items():
                    self._data[key] = self._data.get(key, 0) + int(val)
            else:
                for item in iterable:
                    self._data[item] = self._data.get(item, 0) + 1
        if kwargs:
            for key, val in kwargs.items():
                self._data[key] = self._data.get(key, 0) + int(val)

    def __getitem__(self, key: Any) -> int:
        return self._data.get(key, 0)

    def __setitem__(self, key: Any, value: int) -> None:
        self._data[key] = value

    def items(self):
        return self._data.items()

    def update(
        self, iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None
    ) -> None:
        if iterable is not None:
            if isinstance(iterable, dict) or hasattr(iterable, "items"):
                mapping = cast(Any, iterable)
                for key, val in mapping.items():
                    self._data[key] = self._data.get(key, 0) + int(val)
            else:
                for item in iterable:
                    self._data[item] = self._data.get(item, 0) + 1

    def most_common(self, n: int | None = None):
        items: list[Any] = []
        for pair in self.items():
            items.append(pair)
        count = len(items)
        idx = 1
        while idx < count:
            jdx = idx
            while jdx > 0 and items[jdx - 1][1] < items[jdx][1]:
                items[jdx - 1], items[jdx] = items[jdx], items[jdx - 1]
                jdx -= 1
            idx += 1
        if n is None:
            return items
        return items[:n]


class defaultdict:
    def __init__(self, default_factory=None, *args: Any, **kwargs: Any) -> None:
        self._data: dict = {}
        self.default_factory = default_factory
        if args or kwargs:
            if len(args) > 1:
                raise TypeError("defaultdict expected at most 1 positional argument")
            if args:
                init_arg = args[0]
                if hasattr(init_arg, "items"):
                    for key, val in init_arg.items():
                        self._data[key] = val
                else:
                    for key, val in init_arg:
                        self._data[key] = val
            if kwargs:
                for key, val in kwargs.items():
                    self._data[key] = val

    def __getitem__(self, key: Any) -> Any:
        if key in self._data:
            return self._data[key]
        if self.default_factory is None:
            raise KeyError("missing key")
        if self.default_factory is list:
            value = []
        else:
            value = self.default_factory()
        self._data[key] = value
        return value

    def __setitem__(self, key: Any, value: Any) -> None:
        self._data[key] = value

    def items(self):
        return self._data.items()
