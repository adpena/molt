"""Collections helpers for Molt."""

from __future__ import annotations

from types import NotImplementedType
from typing import Any, Iterable, Iterator, cast

import collections.abc as abc

__all__ = ["abc", "Counter", "defaultdict", "deque"]

_MISSING = object()


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
            items = list(iterable)
            if self._maxlen is not None and len(items) > self._maxlen:
                items = items[-self._maxlen :]
            self._data = items

    def __len__(self) -> int:
        return len(self._data)

    @property
    def maxlen(self) -> int | None:
        return self._maxlen

    def __iter__(self):
        return self._iter_class(self)

    def __repr__(self) -> str:
        if self._maxlen is None:
            return f"deque({list(self)!r})"
        return f"deque({list(self)!r}, maxlen={self._maxlen!r})"

    def __getitem__(self, index: int) -> Any:
        if index < 0:
            index += len(self._data)
        if index < 0 or index >= len(self._data):
            raise IndexError("deque index out of range")
        return self._data[index]

    def __setitem__(self, index: int, value: Any) -> None:
        if index < 0:
            index += len(self._data)
        if index < 0 or index >= len(self._data):
            raise IndexError("deque index out of range")
        self._data[index] = value

    def append(self, item: Any) -> None:
        if self._maxlen is not None and len(self._data) == self._maxlen:
            self._data = self._data[1:]
        self._data = self._data + [item]

    def appendleft(self, item: Any) -> None:
        if self._maxlen is not None and len(self._data) == self._maxlen:
            self._data = self._data[:-1]
        self._data = [item] + self._data

    def pop(self) -> Any:
        if not self._data:
            raise IndexError("pop from an empty deque")
        value = self._data[-1]
        self._data = self._data[:-1]
        return value

    def popleft(self) -> Any:
        if not self._data:
            raise IndexError("pop from an empty deque")
        value = self._data[0]
        self._data = self._data[1:]
        return value

    def rotate(self, n: int = 1) -> None:
        if not self._data:
            return
        length = len(self._data)
        if n == 0:
            return
        n = n % length
        if n:
            self._data = self._data[-n:] + self._data[:-n]

    def clear(self) -> None:
        self._data = []

    def copy(self) -> "deque":
        cls = self.__class__
        cloned = cls()
        cloned._maxlen = self._maxlen
        cloned._data = list(self._data)
        if cloned._maxlen is not None and len(cloned._data) > cloned._maxlen:
            cloned._data = cloned._data[-cloned._maxlen :]
        return cloned

    def count(self, value: Any) -> int:
        count = 0
        for item in self._data:
            if item == value:
                count += 1
        return count

    def index(self, value: Any, start: int = 0, stop: int | None = None) -> int:
        if stop is None:
            stop = len(self._data)
        if start < 0:
            start += len(self._data)
        if stop < 0:
            stop += len(self._data)
        idx = start
        while idx < stop:
            if self._data[idx] == value:
                return idx
            idx += 1
        raise ValueError("deque.index(x): x not in deque")

    def insert(self, index: int, value: Any) -> None:
        if self._maxlen is not None and len(self._data) == self._maxlen:
            raise IndexError("deque already at its maximum size")
        if index < 0:
            index += len(self._data)
        if index < 0:
            index = 0
        if index > len(self._data):
            index = len(self._data)
        self._data = self._data[:index] + [value] + self._data[index:]

    def remove(self, value: Any) -> None:
        idx = 0
        while idx < len(self._data):
            if self._data[idx] == value:
                self._data = self._data[:idx] + self._data[idx + 1 :]
                return
            idx += 1
        raise ValueError("deque.remove(x): x not in deque")

    def extend(self, iterable: Iterable[Any]) -> None:
        items = list(iterable)
        if not items:
            return
        combined = self._data + items
        if self._maxlen is not None and len(combined) > self._maxlen:
            combined = combined[-self._maxlen :]
        self._data = combined

    def extendleft(self, iterable: Iterable[Any]) -> None:
        items = list(iterable)
        if not items:
            return
        items.reverse()
        combined = items + self._data
        if self._maxlen is not None and len(combined) > self._maxlen:
            combined = combined[: self._maxlen]
        self._data = combined

    def reverse(self) -> None:
        self._data = self._data[::-1]

    def __reversed__(self) -> Iterator[Any]:
        return reversed(self._data)


class Counter(dict):
    def __init__(
        self,
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None,
        **kwargs: Any,
    ) -> None:
        self._data: dict[Any, int] = {}
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    self._data[key] = self._data.get(key, 0) + iterable[key]
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    self._data[key] = self._data.get(key, 0) + mapping[key]
            else:
                for item in iterable:
                    self._data[item] = self._data.get(item, 0) + 1
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                self._data[key] = self._data.get(key, 0) + kw_map[key]

    def __missing__(self, key: Any) -> int:
        return 0

    def __getitem__(self, key: Any) -> int:
        return self._data.get(key, 0)

    def __setitem__(self, key: Any, value: int) -> None:
        self._data[key] = value

    def __delitem__(self, key: Any) -> None:
        del self._data[key]

    def __iter__(self):
        return iter(self._data)

    def keys(self):
        return self._data.keys()

    def values(self):
        return self._data.values()

    def __len__(self) -> int:
        return len(self._data)

    def __contains__(self, key: Any) -> bool:
        return key in self._data

    def clear(self) -> None:
        self._data.clear()

    def __repr__(self) -> str:
        if not self._data:
            return "Counter()"
        return f"Counter({self._data!r})"

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, Counter):
            return self._data == other._data
        if isinstance(other, dict):
            return self._data == other
        return False

    def pop(self, key: Any, default: Any = _MISSING) -> Any:
        if default is _MISSING:
            return self._data.pop(key)
        return self._data.pop(key, default)

    def popitem(self):
        return self._data.popitem()

    def setdefault(self, key: Any, default: Any = None) -> Any:
        return self._data.setdefault(key, default)

    def get(self, key: Any, default: int | None = None) -> int | None:
        return self._data.get(key, default)

    def update(self, *args: Any, **kwargs: Any) -> None:
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None
        if args:
            if len(args) > 1:
                raise TypeError(f"update expected at most 1 argument, got {len(args)}")
            iterable = args[0]
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    self._data[key] = self._data.get(key, 0) + iterable[key]
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    self._data[key] = self._data.get(key, 0) + mapping[key]
            else:
                for item in iterable:
                    self._data[item] = self._data.get(item, 0) + 1
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                self._data[key] = self._data.get(key, 0) + kw_map[key]

    def subtract(
        self,
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None,
        **kwargs: Any,
    ) -> None:
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    self._data[key] = self._data.get(key, 0) - iterable[key]
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    self._data[key] = self._data.get(key, 0) - mapping[key]
            else:
                for item in iterable:
                    self._data[item] = self._data.get(item, 0) - 1
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                self._data[key] = self._data.get(key, 0) - kw_map[key]

    def elements(self):
        return _CounterElementsIter(self)

    def items(self):
        return _CounterItemsView(self)

    def most_common(self, n: int | None = None):
        items: list[tuple[Any, int]] = []
        for key in self._data:
            items.append((key, self._data.get(key, 0)))
        items.sort(key=lambda item: item[1], reverse=True)
        if n is None:
            return items
        if n <= 0:
            return []
        return items[:n]

    def total(self):
        total = 0
        for key in self._data:
            total += self._data.get(key, 0)
        return total

    def copy(self) -> "Counter":
        return Counter(self)

    def __add__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self._data:
            count = self._data.get(key, 0) + other._data.get(key, 0)
            if count > 0:
                result[key] = count
        for key in other._data:
            if key in self._data:
                continue
            count = self._data.get(key, 0) + other._data.get(key, 0)
            if count > 0:
                result[key] = count
        return result

    def __sub__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self._data:
            count = self._data.get(key, 0) - other._data.get(key, 0)
            if count > 0:
                result[key] = count
        for key in other._data:
            if key in self._data:
                continue
            count = self._data.get(key, 0) - other._data.get(key, 0)
            if count > 0:
                result[key] = count
        return result

    def __or__(  # type: ignore[override]
        self, other: dict[Any, int]
    ) -> "Counter | NotImplementedType":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self._data:
            count = max(self._data.get(key, 0), other._data.get(key, 0))
            if count > 0:
                result[key] = count
        for key in other._data:
            if key in self._data:
                continue
            count = max(self._data.get(key, 0), other._data.get(key, 0))
            if count > 0:
                result[key] = count
        return result

    def __and__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self._data:
            if key not in other._data:
                continue
            count = min(self._data.get(key, 0), other._data.get(key, 0))
            if count > 0:
                result[key] = count
        return result

    def __iadd__(self, other: "Counter"):
        result = self + other
        if result is NotImplemented:
            return NotImplemented
        self._data = result._data
        return self

    def __isub__(self, other: "Counter"):
        result = self - other
        if result is NotImplemented:
            return NotImplemented
        self._data = result._data
        return self

    def __ior__(  # type: ignore[override]
        self, other: dict[Any, int]
    ) -> "Counter | NotImplementedType":
        result = self | other
        if result is NotImplemented:
            return NotImplemented
        self._data = result._data
        return self

    def __iand__(self, other: "Counter"):
        result = self & other
        if result is NotImplemented:
            return NotImplemented
        self._data = result._data
        return self


class defaultdict(dict):
    def __init__(self, default_factory=None, *args: Any, **kwargs: Any) -> None:
        self.default_factory = default_factory
        self._data: dict[Any, Any] = {}
        if len(args) > 1:
            raise TypeError("defaultdict expected at most 1 positional argument")
        if args:
            init_arg = args[0]
            if isinstance(init_arg, dict):
                for key in init_arg:
                    self._data[key] = init_arg[key]
            elif hasattr(init_arg, "items"):
                for key in init_arg:
                    self._data[key] = init_arg[key]
            else:
                for key, val in init_arg:
                    self._data[key] = val
        if kwargs:
            for key in kwargs:
                self._data[key] = kwargs[key]

    def __getitem__(self, key: Any) -> Any:
        if key in self._data:
            return self._data[key]
        return self.__missing__(key)

    def __setitem__(self, key: Any, value: Any) -> None:
        self._data[key] = value

    def __delitem__(self, key: Any) -> None:
        del self._data[key]

    def __iter__(self):
        return iter(self._data)

    def items(self):
        return self._data.items()

    def keys(self):
        return self._data.keys()

    def values(self):
        return self._data.values()

    def __len__(self) -> int:
        return len(self._data)

    def __contains__(self, key: Any) -> bool:
        return key in self._data

    def get(self, key: Any, default: Any = None) -> Any:
        return self._data.get(key, default)

    def __missing__(self, key: Any) -> Any:
        if self.default_factory is None:
            raise KeyError(key)
        if self.default_factory is list:
            value = []
        elif self.default_factory is dict:
            value = {}
        else:
            value = self.default_factory()
        self[key] = value
        return value

    def __repr__(self) -> str:
        return f"defaultdict({self.default_factory!r}, {dict(self)!r})"


class _CounterElementsIter:
    def __init__(self, counter: Counter) -> None:
        items: list[tuple[Any, int]] = []
        for key in counter._data:
            items.append((key, counter._data.get(key, 0)))
        self._items = items
        self._index = 0
        self._remaining = 0
        self._current_key: Any | None = None

    @staticmethod
    def _coerce_count(count: Any) -> int:
        if isinstance(count, int):
            return int(count)
        if isinstance(count, float):
            raise TypeError(
                f"'{type(count).__name__}' object cannot be interpreted as an integer"
            )
        index = getattr(count, "__index__", None)
        if index is None:
            raise TypeError(
                f"'{type(count).__name__}' object cannot be interpreted as an integer"
            )
        value = index()
        if not isinstance(value, int):
            raise TypeError(
                f"'{type(count).__name__}' object cannot be interpreted as an integer"
            )
        return int(value)

    def __iter__(self):
        return self

    def __next__(self):
        while self._index < len(self._items):
            key, count = self._items[self._index]
            if self._remaining <= 0:
                self._current_key = key
                try:
                    self._remaining = self._coerce_count(count)
                except Exception:
                    raise
            if self._remaining > 0:
                self._remaining -= 1
                if self._remaining == 0:
                    self._index += 1
                return self._current_key
            self._index += 1
        raise StopIteration


class _CounterItemsIter:
    def __init__(self, counter: Counter) -> None:
        items: list[tuple[Any, int]] = []
        for key in counter._data:
            items.append((key, counter._data.get(key, 0)))
        self._items = items
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._index >= len(self._items):
            raise StopIteration
        item = self._items[self._index]
        self._index += 1
        return item


class _CounterItemsView:
    def __init__(self, counter: Counter) -> None:
        self._counter = counter

    def __iter__(self):
        return _CounterItemsIter(self._counter)

    def __len__(self) -> int:
        return len(self._counter._data)

    def __contains__(self, item: Any) -> bool:
        if not isinstance(item, tuple) or len(item) != 2:
            return False
        key, value = item
        return self._counter._data.get(key, 0) == value

    def __repr__(self) -> str:
        return f"dict_items({list(self)!r})"
