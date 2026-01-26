"""Collections helpers for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Iterable, Iterator, cast

import collections.abc as abc
import builtins as _builtins
import keyword as _keyword
import operator as _operator

__all__ = ["abc", "Counter", "defaultdict", "deque", "namedtuple"]

_MISSING = object()

if TYPE_CHECKING:
    from types import NotImplementedType


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


_MOLT_CLASS_NEW = _load_intrinsic("_molt_class_new")
_MOLT_CLASS_SET_BASE = _load_intrinsic("_molt_class_set_base")
_MOLT_CLASS_APPLY_SET_NAME = _load_intrinsic("_molt_class_apply_set_name")


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


def namedtuple(
    typename: Any,
    field_names: Any,
    *,
    rename: bool = False,
    defaults: Iterable[Any] | None = None,
    module: str | None = None,
):
    typename = str(typename)
    if isinstance(field_names, str):
        field_names = field_names.replace(",", " ").split()
    else:
        field_names = [str(name) for name in field_names]

    if not typename.isidentifier() or _keyword.iskeyword(typename):
        raise ValueError(
            f"Type names and field names must be valid identifiers: {typename!r}"
        )

    seen: set[str] = set()
    normalized: list[str] = []
    for idx, name in enumerate(field_names):
        invalid = (
            (not name.isidentifier())
            or _keyword.iskeyword(name)
            or name.startswith("_")
            or name in seen
        )
        if invalid:
            if rename:
                name = f"_{idx}"
            else:
                if name in seen:
                    raise ValueError(f"Encountered duplicate field name: {name!r}")
                if name.startswith("_"):
                    raise ValueError(
                        f"Field names cannot start with an underscore: {name!r}"
                    )
                raise ValueError(
                    f"Type names and field names must be valid identifiers: {name!r}"
                )
        if name in seen:
            raise ValueError(f"Encountered duplicate field name: {name!r}")
        seen.add(name)
        normalized.append(name)

    field_names = normalized
    field_tuple = tuple(field_names)
    num_fields = len(field_tuple)
    field_index = {name: idx for idx, name in enumerate(field_tuple)}

    defaults_tuple: tuple[Any, ...] | None = None
    if defaults is not None:
        defaults_tuple = tuple(defaults)
        if len(defaults_tuple) > num_fields:
            raise TypeError("Got more default values than field names")
    field_defaults: dict[str, Any] = {}
    if defaults_tuple:
        for name, value in zip(field_tuple[-len(defaults_tuple) :], defaults_tuple):
            field_defaults[name] = value

    if module is None:
        try:
            import sys as _sys

            module = _sys._getframe(1).f_globals.get("__name__", "__main__")
        except Exception:
            module = "__main__"

    if not callable(_MOLT_CLASS_NEW) or not callable(_MOLT_CLASS_SET_BASE):
        raise NotImplementedError("namedtuple requires Molt runtime support")

    def __new__(cls, *args: Any, **kwargs: Any) -> Any:
        if len(args) > num_fields:
            raise TypeError(f"Expected {num_fields} arguments, got {len(args)}")
        values = [_MISSING] * num_fields
        for idx, value in enumerate(args):
            values[idx] = value
        for name, value in kwargs.items():
            idx = field_index.get(name)
            if idx is None:
                raise TypeError(f"Got unexpected field names: {[name]!r}")
            if values[idx] is not _MISSING:
                raise TypeError(f"Got multiple values for field name: {name!r}")
            values[idx] = value
        if defaults_tuple:
            start = num_fields - len(defaults_tuple)
            for idx, default in enumerate(defaults_tuple, start=start):
                if values[idx] is _MISSING:
                    values[idx] = default
        missing = [
            field_tuple[i] for i, value in enumerate(values) if value is _MISSING
        ]
        if missing:
            raise TypeError(
                f"Expected {num_fields} arguments, got {num_fields - len(missing)}"
            )
        return tuple.__new__(cls, tuple(values))

    if defaults_tuple:
        __new__.__defaults__ = defaults_tuple

    def _make(cls, iterable: Iterable[Any]) -> Any:
        items = tuple(iterable)
        if len(items) != num_fields:
            raise TypeError(f"Expected {num_fields} arguments, got {len(items)}")
        return cls(*items)

    def _replace(self, **kwds: Any) -> Any:
        unexpected = [name for name in kwds if name not in field_index]
        if unexpected:
            raise TypeError(f"Got unexpected field names: {unexpected!r}")
        values = [kwds.get(name, getattr(self, name)) for name in field_tuple]
        return type(self)(*values)

    def _asdict(self) -> dict[str, Any]:
        return {name: value for name, value in zip(field_tuple, self)}

    def __getnewargs__(self) -> tuple[Any, ...]:
        return tuple(self)

    def __repr__(self) -> str:
        if not field_tuple:
            return f"{typename}()"
        items = ", ".join(f"{name}={getattr(self, name)!r}" for name in field_tuple)
        return f"{typename}({items})"

    cls = _MOLT_CLASS_NEW(typename)
    base_res = _MOLT_CLASS_SET_BASE(cls, tuple)
    if base_res is not None:
        cls = base_res
    setattr(cls, "__slots__", ())
    setattr(cls, "__doc__", f"{typename}({', '.join(field_tuple)})")
    setattr(cls, "__module__", module)
    setattr(cls, "__qualname__", typename)
    setattr(cls, "_fields", field_tuple)
    setattr(cls, "_field_defaults", field_defaults)
    setattr(cls, "__match_args__", field_tuple)
    setattr(cls, "__new__", __new__)
    setattr(cls, "_make", classmethod(_make))
    setattr(cls, "_replace", _replace)
    setattr(cls, "_asdict", _asdict)
    setattr(cls, "__getnewargs__", __getnewargs__)
    setattr(cls, "__repr__", __repr__)
    for idx, name in enumerate(field_tuple):
        setattr(cls, name, property(_operator.itemgetter(idx)))
    if callable(_MOLT_CLASS_APPLY_SET_NAME):
        _MOLT_CLASS_APPLY_SET_NAME(cls)
    return cls


class Counter(dict):
    def __init__(
        self,
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None,
        **kwargs: Any,
    ) -> None:
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    dict.__setitem__(self, key, dict.get(self, key, 0) + iterable[key])
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    dict.__setitem__(self, key, dict.get(self, key, 0) + mapping[key])
            else:
                for item in iterable:
                    dict.__setitem__(self, item, dict.get(self, item, 0) + 1)
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                dict.__setitem__(self, key, dict.get(self, key, 0) + kw_map[key])

    def __missing__(self, key: Any) -> int:
        return 0

    def __getitem__(self, key: Any) -> int:
        if key in self:
            return dict.__getitem__(self, key)
        return 0

    def __setitem__(self, key: Any, value: int) -> None:
        dict.__setitem__(self, key, value)

    def __delitem__(self, key: Any) -> None:
        dict.__delitem__(self, key)

    def __iter__(self):
        return dict.__iter__(self)

    def keys(self):
        return dict.keys(self)

    def values(self):
        return dict.values(self)

    def __len__(self) -> int:
        return dict.__len__(self)

    def __contains__(self, key: Any) -> bool:
        return dict.__contains__(self, key)

    def clear(self) -> None:
        keys = list(self)
        for key in keys:
            dict.__delitem__(self, key)

    def __repr__(self) -> str:
        if len(self) == 0:
            return "Counter()"
        items: list[str] = []
        for key in self:
            items.append(f"{key!r}: {dict.get(self, key, 0)!r}")
        return f"Counter({{{', '.join(items)}}})"

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, dict):
            return False
        if len(self) != len(other):
            return False
        for key in self:
            if dict.get(self, key, 0) != dict.get(other, key, _MISSING):
                return False
        for key in other:
            if key not in self:
                return False
        return True

    def pop(self, key: Any, default: Any = _MISSING) -> Any:
        if key in self:
            val = dict.__getitem__(self, key)
            dict.__delitem__(self, key)
            return val
        if default is _MISSING:
            raise KeyError(key)
        return default

    def popitem(self):
        last_key = _MISSING
        for key in self:
            last_key = key
        if last_key is _MISSING:
            raise KeyError("popitem(): dictionary is empty")
        val = dict.__getitem__(self, last_key)
        dict.__delitem__(self, last_key)
        return (last_key, val)

    def setdefault(self, key: Any, default: Any = None) -> Any:
        if key in self:
            return dict.__getitem__(self, key)
        dict.__setitem__(self, key, default)
        return default

    def get(self, key: Any, default: int | None = None) -> int | None:
        if key in self:
            return dict.__getitem__(self, key)
        return default

    def update(self, *args: Any, **kwargs: Any) -> None:
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None
        if args:
            if len(args) > 1:
                raise TypeError(f"update expected at most 1 argument, got {len(args)}")
            iterable = args[0]
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    dict.__setitem__(self, key, dict.get(self, key, 0) + iterable[key])
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    dict.__setitem__(self, key, dict.get(self, key, 0) + mapping[key])
            else:
                for item in iterable:
                    dict.__setitem__(self, item, dict.get(self, item, 0) + 1)
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                dict.__setitem__(self, key, dict.get(self, key, 0) + kw_map[key])

    def subtract(
        self,
        iterable: abc.Mapping[Any, Any] | Iterable[Any] | None = None,
        **kwargs: Any,
    ) -> None:
        if iterable is not None:
            if isinstance(iterable, dict):
                for key in iterable:
                    dict.__setitem__(self, key, dict.get(self, key, 0) - iterable[key])
            elif hasattr(iterable, "items"):
                mapping = cast(abc.Mapping[Any, Any], iterable)
                for key in mapping:
                    dict.__setitem__(self, key, dict.get(self, key, 0) - mapping[key])
            else:
                for item in iterable:
                    dict.__setitem__(self, item, dict.get(self, item, 0) - 1)
        if kwargs:
            kw_map: dict[str, Any] = kwargs
            for key in kw_map:
                dict.__setitem__(self, key, dict.get(self, key, 0) - kw_map[key])

    def elements(self):
        return _CounterElementsIter(self)

    def items(self):
        return _CounterItemsView(self)

    def most_common(self, n: int | None = None):
        items: list[tuple[Any, int]] = []
        for key in self:
            items.append((key, dict.get(self, key, 0)))
        items.sort(key=lambda item: item[1], reverse=True)
        if n is None:
            return items
        if n <= 0:
            return []
        return items[:n]

    def total(self):
        total = 0
        for key in self:
            total += dict.get(self, key, 0)
        return total

    def copy(self) -> "Counter":
        return Counter(self)

    def __add__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self:
            count = dict.get(self, key, 0) + dict.get(other, key, 0)
            if count > 0:
                result[key] = count
        for key in other:
            if key in self:
                continue
            count = dict.get(self, key, 0) + dict.get(other, key, 0)
            if count > 0:
                result[key] = count
        return result

    def __sub__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self:
            count = dict.get(self, key, 0) - dict.get(other, key, 0)
            if count > 0:
                result[key] = count
        for key in other:
            if key in self:
                continue
            count = dict.get(self, key, 0) - dict.get(other, key, 0)
            if count > 0:
                result[key] = count
        return result

    def __or__(  # type: ignore[override]
        self, other: dict[Any, int]
    ) -> "Counter | NotImplementedType":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self:
            count = max(dict.get(self, key, 0), dict.get(other, key, 0))
            if count > 0:
                result[key] = count
        for key in other:
            if key in self:
                continue
            count = max(dict.get(self, key, 0), dict.get(other, key, 0))
            if count > 0:
                result[key] = count
        return result

    def __and__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter(())
        for key in self:
            if key not in other:
                continue
            count = min(dict.get(self, key, 0), dict.get(other, key, 0))
            if count > 0:
                result[key] = count
        return result

    def __iadd__(self, other: "Counter"):
        result = self + other
        if result is NotImplemented:
            return NotImplemented
        self.clear()
        for key, value in result.items():
            dict.__setitem__(self, key, value)
        return self

    def __isub__(self, other: "Counter"):
        result = self - other
        if result is NotImplemented:
            return NotImplemented
        self.clear()
        for key, value in result.items():
            dict.__setitem__(self, key, value)
        return self

    def __ior__(  # type: ignore[override]
        self, other: dict[Any, int]
    ) -> "Counter | NotImplementedType":
        result = self | other
        if result is NotImplemented:
            return NotImplemented
        self.clear()
        for key, value in result.items():
            dict.__setitem__(self, key, value)
        return self

    def __iand__(self, other: "Counter"):
        result = self & other
        if result is NotImplemented:
            return NotImplemented
        self.clear()
        for key, value in result.items():
            dict.__setitem__(self, key, value)
        return self


class defaultdict(dict):
    def __init__(self, default_factory=None, *args: Any, **kwargs: Any) -> None:
        self.default_factory = default_factory
        if len(args) > 1:
            raise TypeError("defaultdict expected at most 1 positional argument")
        if args:
            dict.update(self, args[0])
        if kwargs:
            dict.update(self, kwargs)

    def __getitem__(self, key: Any) -> Any:
        try:
            return dict.__getitem__(self, key)
        except KeyError:
            return self.__missing__(key)

    def __setitem__(self, key: Any, value: Any) -> None:
        dict.__setitem__(self, key, value)

    def __delitem__(self, key: Any) -> None:
        dict.__delitem__(self, key)

    def __iter__(self):
        return dict.__iter__(self)

    def items(self):
        return dict.items(self)

    def keys(self):
        return dict.keys(self)

    def values(self):
        return dict.values(self)

    def __len__(self) -> int:
        return dict.__len__(self)

    def __contains__(self, key: Any) -> bool:
        return dict.__contains__(self, key)

    def get(self, key: Any, default: Any = None) -> Any:
        return dict.get(self, key, default)

    def __missing__(self, key: Any) -> Any:
        if self.default_factory is None:
            raise KeyError(key)
        if self.default_factory is list:
            value = []
        elif self.default_factory is dict:
            value = {}
        else:
            value = self.default_factory()
        dict.__setitem__(self, key, value)
        return value

    def __repr__(self) -> str:
        return f"defaultdict({self.default_factory!r}, {dict(self)!r})"


class _CounterElementsIter:
    def __init__(self, counter: Counter) -> None:
        items: list[tuple[Any, int]] = []
        for key in counter:
            items.append((key, dict.get(counter, key, 0)))
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
        for key in counter:
            items.append((key, dict.get(counter, key, 0)))
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
        return len(self._counter)

    def __contains__(self, item: Any) -> bool:
        if not isinstance(item, tuple) or len(item) != 2:
            return False
        key, value = item
        return dict.get(self._counter, key, 0) == value

    def __repr__(self) -> str:
        return f"dict_items({list(self)!r})"
