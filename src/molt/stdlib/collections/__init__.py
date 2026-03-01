"""Collections helpers for Molt (intrinsic-backed)."""

from __future__ import annotations

import sys as _sys

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from typing import Any, Iterable, Iterator, cast
else:
    Any = object()
    Iterable = object()
    Iterator = object()

    def cast(_tp, value):
        return value


import collections.abc as abc
import keyword as _keyword

from _intrinsics import require_intrinsic as _require_intrinsic

# Re-export OrderedDict from _collections
from _collections import OrderedDict

__all__ = [
    "abc",
    "ChainMap",
    "Counter",
    "defaultdict",
    "deque",
    "namedtuple",
    "OrderedDict",
]

_MISSING = object()

# --- Class-building intrinsics (for namedtuple) ---
_MOLT_CLASS_NEW = _require_intrinsic("molt_class_new", globals())
_MOLT_CLASS_SET_BASE = _require_intrinsic("molt_class_set_base", globals())
_MOLT_CLASS_APPLY_SET_NAME = _require_intrinsic("molt_class_apply_set_name", globals())

# --- deque intrinsics ---
_MOLT_DEQUE_NEW = _require_intrinsic("molt_deque_new", globals())
_MOLT_DEQUE_FROM_ITERABLE = _require_intrinsic("molt_deque_from_iterable", globals())
_MOLT_DEQUE_APPEND = _require_intrinsic("molt_deque_append", globals())
_MOLT_DEQUE_APPENDLEFT = _require_intrinsic("molt_deque_appendleft", globals())
_MOLT_DEQUE_CLEAR = _require_intrinsic("molt_deque_clear", globals())
_MOLT_DEQUE_CONTAINS = _require_intrinsic("molt_deque_contains", globals())
_MOLT_DEQUE_COPY = _require_intrinsic("molt_deque_copy", globals())
_MOLT_DEQUE_COUNT = _require_intrinsic("molt_deque_count", globals())
_MOLT_DEQUE_DELITEM = _require_intrinsic("molt_deque_delitem", globals())
_MOLT_DEQUE_DROP = _require_intrinsic("molt_deque_drop", globals())
_MOLT_DEQUE_EXTEND = _require_intrinsic("molt_deque_extend", globals())
_MOLT_DEQUE_EXTENDLEFT = _require_intrinsic("molt_deque_extendleft", globals())
_MOLT_DEQUE_GETITEM = _require_intrinsic("molt_deque_getitem", globals())
_MOLT_DEQUE_INDEX = _require_intrinsic("molt_deque_index", globals())
_MOLT_DEQUE_INSERT = _require_intrinsic("molt_deque_insert", globals())
_MOLT_DEQUE_LEN = _require_intrinsic("molt_deque_len", globals())
_MOLT_DEQUE_MAXLEN = _require_intrinsic("molt_deque_maxlen", globals())
_MOLT_DEQUE_POP = _require_intrinsic("molt_deque_pop", globals())
_MOLT_DEQUE_POPLEFT = _require_intrinsic("molt_deque_popleft", globals())
_MOLT_DEQUE_REMOVE = _require_intrinsic("molt_deque_remove", globals())
_MOLT_DEQUE_REVERSE = _require_intrinsic("molt_deque_reverse", globals())
_MOLT_DEQUE_ROTATE = _require_intrinsic("molt_deque_rotate", globals())
_MOLT_DEQUE_SETITEM = _require_intrinsic("molt_deque_setitem", globals())

# --- Counter intrinsics ---
_MOLT_COUNTER_ADD = _require_intrinsic("molt_counter_add", globals())
_MOLT_COUNTER_AND = _require_intrinsic("molt_counter_and", globals())
_MOLT_COUNTER_CLEAR = _require_intrinsic("molt_counter_clear", globals())
_MOLT_COUNTER_CONTAINS = _require_intrinsic("molt_counter_contains", globals())
_MOLT_COUNTER_COPY = _require_intrinsic("molt_counter_copy", globals())
_MOLT_COUNTER_DELITEM = _require_intrinsic("molt_counter_delitem", globals())
_MOLT_COUNTER_DROP = _require_intrinsic("molt_counter_drop", globals())
_MOLT_COUNTER_ELEMENTS = _require_intrinsic("molt_counter_elements", globals())
_MOLT_COUNTER_FROM_ITERABLE = _require_intrinsic(
    "molt_counter_from_iterable", globals()
)
_MOLT_COUNTER_FROM_MAPPING = _require_intrinsic("molt_counter_from_mapping", globals())
_MOLT_COUNTER_GETITEM = _require_intrinsic("molt_counter_getitem", globals())
_MOLT_COUNTER_ITEMS = _require_intrinsic("molt_counter_items", globals())
_MOLT_COUNTER_LEN = _require_intrinsic("molt_counter_len", globals())
_MOLT_COUNTER_MOST_COMMON = _require_intrinsic("molt_counter_most_common", globals())
_MOLT_COUNTER_NEW = _require_intrinsic("molt_counter_new", globals())
_MOLT_COUNTER_OR = _require_intrinsic("molt_counter_or", globals())
_MOLT_COUNTER_POP = _require_intrinsic("molt_counter_pop", globals())
_MOLT_COUNTER_SETITEM = _require_intrinsic("molt_counter_setitem", globals())
_MOLT_COUNTER_SUB = _require_intrinsic("molt_counter_sub", globals())
_MOLT_COUNTER_SUBTRACT = _require_intrinsic("molt_counter_subtract", globals())
_MOLT_COUNTER_TOTAL = _require_intrinsic("molt_counter_total", globals())
_MOLT_COUNTER_UPDATE = _require_intrinsic("molt_counter_update", globals())

# --- defaultdict intrinsics ---
_MOLT_DEFAULTDICT_COPY = _require_intrinsic("molt_defaultdict_copy", globals())
_MOLT_DEFAULTDICT_DROP = _require_intrinsic("molt_defaultdict_drop", globals())
_MOLT_DEFAULTDICT_FACTORY = _require_intrinsic("molt_defaultdict_factory", globals())
_MOLT_DEFAULTDICT_MISSING = _require_intrinsic("molt_defaultdict_missing", globals())
_MOLT_DEFAULTDICT_NEW = _require_intrinsic("molt_defaultdict_new", globals())

# --- ChainMap intrinsics ---
_MOLT_CHAINMAP_CONTAINS = _require_intrinsic("molt_chainmap_contains", globals())
_MOLT_CHAINMAP_DELITEM = _require_intrinsic("molt_chainmap_delitem", globals())
_MOLT_CHAINMAP_DROP = _require_intrinsic("molt_chainmap_drop", globals())
_MOLT_CHAINMAP_GETITEM = _require_intrinsic("molt_chainmap_getitem", globals())
_MOLT_CHAINMAP_KEYS = _require_intrinsic("molt_chainmap_keys", globals())
_MOLT_CHAINMAP_LEN = _require_intrinsic("molt_chainmap_len", globals())
_MOLT_CHAINMAP_MAPS = _require_intrinsic("molt_chainmap_maps", globals())
_MOLT_CHAINMAP_NEW = _require_intrinsic("molt_chainmap_new", globals())
_MOLT_CHAINMAP_NEW_CHILD = _require_intrinsic("molt_chainmap_new_child", globals())
_MOLT_CHAINMAP_PARENTS = _require_intrinsic("molt_chainmap_parents", globals())
_MOLT_CHAINMAP_SETITEM = _require_intrinsic("molt_chainmap_setitem", globals())


# ---------------------------------------------------------------------------
# deque — fully intrinsic-backed
# ---------------------------------------------------------------------------


class _DequeIter:
    """Forward iterator over an intrinsic-backed deque."""

    __slots__ = ("_deque", "_index")

    def __init__(self, deq: "deque") -> None:
        self._deque = deq
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self) -> Any:
        if self._index >= len(self._deque):
            raise StopIteration
        value = _MOLT_DEQUE_GETITEM(self._deque._handle, self._index)
        self._index += 1
        return value


class _DequeRevIter:
    """Reverse iterator over an intrinsic-backed deque."""

    __slots__ = ("_deque", "_index")

    def __init__(self, deq: "deque") -> None:
        self._deque = deq
        self._index = len(deq) - 1

    def __iter__(self):
        return self

    def __next__(self) -> Any:
        if self._index < 0:
            raise StopIteration
        value = _MOLT_DEQUE_GETITEM(self._deque._handle, self._index)
        self._index -= 1
        return value


class deque:
    __slots__ = ("_handle",)

    def __init__(
        self, iterable: Iterable[Any] | None = None, maxlen: int | None = None
    ):
        if maxlen is not None and maxlen < 0:
            raise ValueError("maxlen must be non-negative")
        if iterable is not None:
            if isinstance(iterable, (list, tuple)):
                self._handle = _MOLT_DEQUE_FROM_ITERABLE(iterable, maxlen)
            else:
                self._handle = _MOLT_DEQUE_FROM_ITERABLE(list(iterable), maxlen)
        else:
            self._handle = _MOLT_DEQUE_NEW(maxlen)

    @classmethod
    def _from_handle(cls, handle) -> "deque":
        inst = cls.__new__(cls)
        inst._handle = handle
        return inst

    def __len__(self) -> int:
        return int(_MOLT_DEQUE_LEN(self._handle))

    @property
    def maxlen(self) -> int | None:
        result = _MOLT_DEQUE_MAXLEN(self._handle)
        if result is None:
            return None
        return int(result)

    def __iter__(self):
        return _DequeIter(self)

    def __reversed__(self) -> Iterator[Any]:
        return _DequeRevIter(self)

    def __repr__(self) -> str:
        items = list(self)
        ml = self.maxlen
        if ml is None:
            return f"deque({items!r})"
        return f"deque({items!r}, maxlen={ml!r})"

    def __bool__(self) -> bool:
        return len(self) > 0

    def __contains__(self, item) -> bool:
        return bool(_MOLT_DEQUE_CONTAINS(self._handle, item))

    def __getitem__(self, index: int) -> Any:
        return _MOLT_DEQUE_GETITEM(self._handle, index)

    def __setitem__(self, index: int, value: Any) -> None:
        _MOLT_DEQUE_SETITEM(self._handle, index, value)

    def __delitem__(self, index: int) -> None:
        _MOLT_DEQUE_DELITEM(self._handle, index)

    def append(self, item: Any) -> None:
        _MOLT_DEQUE_APPEND(self._handle, item)

    def appendleft(self, item: Any) -> None:
        _MOLT_DEQUE_APPENDLEFT(self._handle, item)

    def pop(self) -> Any:
        return _MOLT_DEQUE_POP(self._handle)

    def popleft(self) -> Any:
        return _MOLT_DEQUE_POPLEFT(self._handle)

    def rotate(self, n: int = 1) -> None:
        _MOLT_DEQUE_ROTATE(self._handle, n)

    def clear(self) -> None:
        _MOLT_DEQUE_CLEAR(self._handle)

    def copy(self) -> "deque":
        return deque._from_handle(_MOLT_DEQUE_COPY(self._handle))

    def count(self, value: Any) -> int:
        return int(_MOLT_DEQUE_COUNT(self._handle, value))

    def index(self, value: Any, start: int = 0, stop: int | None = None) -> int:
        if stop is None:
            stop = len(self)
        return int(_MOLT_DEQUE_INDEX(self._handle, value, start, stop))

    def insert(self, index: int, value: Any) -> None:
        _MOLT_DEQUE_INSERT(self._handle, index, value)

    def remove(self, value: Any) -> None:
        _MOLT_DEQUE_REMOVE(self._handle, value)

    def extend(self, iterable: Iterable[Any]) -> None:
        if isinstance(iterable, (list, tuple)):
            _MOLT_DEQUE_EXTEND(self._handle, iterable)
        else:
            _MOLT_DEQUE_EXTEND(self._handle, list(iterable))

    def extendleft(self, iterable: Iterable[Any]) -> None:
        if isinstance(iterable, (list, tuple)):
            _MOLT_DEQUE_EXTENDLEFT(self._handle, iterable)
        else:
            _MOLT_DEQUE_EXTENDLEFT(self._handle, list(iterable))

    def reverse(self) -> None:
        _MOLT_DEQUE_REVERSE(self._handle)

    def __eq__(self, other) -> bool:
        if not isinstance(other, deque):
            return NotImplemented
        if len(self) != len(other):
            return False
        for a, b in zip(self, other):
            if a != b:
                return False
        return True

    def __ne__(self, other) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    def __lt__(self, other) -> bool:
        if not isinstance(other, deque):
            return NotImplemented
        return list(self) < list(other)

    def __le__(self, other) -> bool:
        if not isinstance(other, deque):
            return NotImplemented
        return list(self) <= list(other)

    def __gt__(self, other) -> bool:
        if not isinstance(other, deque):
            return NotImplemented
        return list(self) > list(other)

    def __ge__(self, other) -> bool:
        if not isinstance(other, deque):
            return NotImplemented
        return list(self) >= list(other)

    def __add__(self, other):
        if not isinstance(other, deque):
            return NotImplemented
        result = self.copy()
        result.extend(other)
        return result

    def __iadd__(self, other):
        self.extend(other)
        return self

    def __mul__(self, n):
        if not isinstance(n, int):
            return NotImplemented
        items = list(self) * n
        ml = self.maxlen
        return deque(items, maxlen=ml)

    def __imul__(self, n):
        if not isinstance(n, int):
            return NotImplemented
        items = list(self) * n
        ml = self.maxlen
        self.clear()
        if ml is not None and len(items) > ml:
            items = items[-ml:]
        for item in items:
            self.append(item)
        return self

    def __hash__(self):
        raise TypeError("unhashable type: 'deque'")

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_DEQUE_DROP(handle)
            except Exception:
                pass


# ---------------------------------------------------------------------------
# namedtuple — kept as-is (pure Python, no intrinsic needed)
# ---------------------------------------------------------------------------


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
            module = _sys._getframe(1).f_globals.get("__name__", "__main__")
        except Exception:
            module = "__main__"

    use_intrinsics = callable(_MOLT_CLASS_NEW) and callable(_MOLT_CLASS_SET_BASE)

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

    def _field_getter(index: int):
        def _getter(self):
            return self[index]

        return _getter

    if use_intrinsics:
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
            setattr(cls, name, property(_field_getter(idx)))
        if callable(_MOLT_CLASS_APPLY_SET_NAME):
            _MOLT_CLASS_APPLY_SET_NAME(cls)
        return cls

    namespace: dict[str, Any] = {
        "__slots__": (),
        "__doc__": f"{typename}({', '.join(field_tuple)})",
        "__module__": module,
        "__qualname__": typename,
        "_fields": field_tuple,
        "_field_defaults": field_defaults,
        "__match_args__": field_tuple,
        "__new__": __new__,
        "_make": classmethod(_make),
        "_replace": _replace,
        "_asdict": _asdict,
        "__getnewargs__": __getnewargs__,
        "__repr__": __repr__,
    }
    for idx, name in enumerate(field_tuple):
        namespace[name] = property(_field_getter(idx))
    cls = type.__new__(type, typename, (tuple,), namespace)
    type.__init__(cls, typename, (tuple,), namespace)
    return cls


# ---------------------------------------------------------------------------
# Counter — intrinsic-backed (handle-based, NOT a dict subclass)
# ---------------------------------------------------------------------------
# NOTE: isinstance(counter, dict) is False. This is a known Molt breakage
# since Counter uses handle-based storage delegated to Rust intrinsics.
# ---------------------------------------------------------------------------


class _CounterElementsIter:
    """Iterator for Counter.elements() backed by intrinsic."""

    __slots__ = ("_items", "_index", "_remaining", "_current_key")

    def __init__(self, counter: "Counter") -> None:
        self._items = _MOLT_COUNTER_ITEMS(counter._handle)
        self._index = 0
        self._remaining = 0
        self._current_key = None

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
    __slots__ = ("_items", "_index")

    def __init__(self, items) -> None:
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
    __slots__ = ("_counter",)

    def __init__(self, counter: "Counter") -> None:
        self._counter = counter

    def __iter__(self):
        return _CounterItemsIter(_MOLT_COUNTER_ITEMS(self._counter._handle))

    def __len__(self) -> int:
        return len(self._counter)

    def __contains__(self, item: Any) -> bool:
        if not isinstance(item, tuple) or len(item) != 2:
            return False
        key, value = item
        return _MOLT_COUNTER_GETITEM(self._counter._handle, key) == value

    def __repr__(self) -> str:
        return f"dict_items({list(self)!r})"


class _CounterKeysIter:
    __slots__ = ("_items", "_index")

    def __init__(self, items) -> None:
        self._items = items
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._index >= len(self._items):
            raise StopIteration
        key, _count = self._items[self._index]
        self._index += 1
        return key


class _CounterValuesIter:
    __slots__ = ("_items", "_index")

    def __init__(self, items) -> None:
        self._items = items
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._index >= len(self._items):
            raise StopIteration
        _key, count = self._items[self._index]
        self._index += 1
        return count


class Counter:
    __slots__ = ("_handle",)

    def __init__(
        self,
        iterable=None,
        **kwargs,
    ) -> None:
        if iterable is not None:
            if isinstance(iterable, dict) or hasattr(iterable, "items"):
                if isinstance(iterable, dict):
                    pairs = list(iterable.items())
                else:
                    pairs = [(k, iterable[k]) for k in iterable]
                self._handle = _MOLT_COUNTER_FROM_MAPPING(pairs)
            else:
                if isinstance(iterable, (list, tuple)):
                    self._handle = _MOLT_COUNTER_FROM_ITERABLE(iterable)
                else:
                    self._handle = _MOLT_COUNTER_FROM_ITERABLE(list(iterable))
        else:
            self._handle = _MOLT_COUNTER_NEW()
        if kwargs:
            _MOLT_COUNTER_UPDATE(self._handle, list(kwargs.items()))

    @classmethod
    def _from_handle(cls, handle) -> "Counter":
        inst = cls.__new__(cls)
        inst._handle = handle
        return inst

    def __missing__(self, key: Any) -> int:
        return 0

    def __getitem__(self, key: Any) -> int:
        return _MOLT_COUNTER_GETITEM(self._handle, key)

    def __setitem__(self, key: Any, value: int) -> None:
        _MOLT_COUNTER_SETITEM(self._handle, key, value)

    def __delitem__(self, key: Any) -> None:
        _MOLT_COUNTER_DELITEM(self._handle, key)

    def __contains__(self, key: Any) -> bool:
        return bool(_MOLT_COUNTER_CONTAINS(self._handle, key))

    def __len__(self) -> int:
        return int(_MOLT_COUNTER_LEN(self._handle))

    def __bool__(self) -> bool:
        return len(self) > 0

    def __iter__(self):
        return _CounterKeysIter(_MOLT_COUNTER_ITEMS(self._handle))

    def keys(self):
        items = _MOLT_COUNTER_ITEMS(self._handle)
        return [k for k, _v in items]

    def values(self):
        items = _MOLT_COUNTER_ITEMS(self._handle)
        return [v for _k, v in items]

    def items(self):
        return _CounterItemsView(self)

    def get(self, key: Any, default=None):
        if _MOLT_COUNTER_CONTAINS(self._handle, key):
            return _MOLT_COUNTER_GETITEM(self._handle, key)
        return default

    def pop(self, key: Any, *args):
        if len(args) > 1:
            raise TypeError(f"pop expected at most 2 arguments, got {1 + len(args)}")
        if _MOLT_COUNTER_CONTAINS(self._handle, key):
            count = _MOLT_COUNTER_GETITEM(self._handle, key)
            _MOLT_COUNTER_DELITEM(self._handle, key)
            return count
        if args:
            return args[0]
        raise KeyError(key)

    def popitem(self):
        if not len(self):
            raise KeyError("popitem(): dictionary is empty")
        items = _MOLT_COUNTER_ITEMS(self._handle)
        key, count = items[-1]
        _MOLT_COUNTER_DELITEM(self._handle, key)
        return (key, count)

    def setdefault(self, key: Any, default: Any = None) -> Any:
        if _MOLT_COUNTER_CONTAINS(self._handle, key):
            return _MOLT_COUNTER_GETITEM(self._handle, key)
        _MOLT_COUNTER_SETITEM(self._handle, key, default)
        return default

    def clear(self) -> None:
        _MOLT_COUNTER_CLEAR(self._handle)

    def copy(self) -> "Counter":
        return Counter._from_handle(_MOLT_COUNTER_COPY(self._handle))

    def update(self, *args, **kwargs) -> None:
        if args:
            if len(args) > 1:
                raise TypeError(f"update expected at most 1 argument, got {len(args)}")
            source = args[0]
            if isinstance(source, Counter):
                _MOLT_COUNTER_UPDATE(self._handle, _MOLT_COUNTER_ITEMS(source._handle))
            elif isinstance(source, dict) or hasattr(source, "items"):
                if isinstance(source, dict):
                    pairs = list(source.items())
                else:
                    pairs = [(k, source[k]) for k in source]
                _MOLT_COUNTER_UPDATE(self._handle, pairs)
            else:
                if isinstance(source, (list, tuple)):
                    _MOLT_COUNTER_UPDATE(self._handle, source)
                else:
                    _MOLT_COUNTER_UPDATE(self._handle, list(source))
        if kwargs:
            _MOLT_COUNTER_UPDATE(self._handle, list(kwargs.items()))

    def subtract(self, iterable=None, **kwargs) -> None:
        if iterable is not None:
            if isinstance(iterable, Counter):
                _MOLT_COUNTER_SUBTRACT(
                    self._handle, _MOLT_COUNTER_ITEMS(iterable._handle)
                )
            elif isinstance(iterable, dict) or hasattr(iterable, "items"):
                if isinstance(iterable, dict):
                    pairs = list(iterable.items())
                else:
                    pairs = [(k, iterable[k]) for k in iterable]
                _MOLT_COUNTER_SUBTRACT(self._handle, pairs)
            else:
                if isinstance(iterable, (list, tuple)):
                    _MOLT_COUNTER_SUBTRACT(self._handle, iterable)
                else:
                    _MOLT_COUNTER_SUBTRACT(self._handle, list(iterable))
        if kwargs:
            _MOLT_COUNTER_SUBTRACT(self._handle, list(kwargs.items()))

    def elements(self):
        return _CounterElementsIter(self)

    def most_common(self, n: int | None = None):
        return _MOLT_COUNTER_MOST_COMMON(self._handle, n)

    def total(self):
        return _MOLT_COUNTER_TOTAL(self._handle)

    def __repr__(self) -> str:
        if len(self) == 0:
            return "Counter()"
        mc = self.most_common()
        items: list[str] = []
        for key, count in mc:
            items.append(f"{key!r}: {count!r}")
        return f"Counter({{{', '.join(items)}}})"

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, Counter):
            if len(self) != len(other):
                return False
            for key in self:
                if self[key] != other[key]:
                    return False
            for key in other:
                if key not in self:
                    return False
            return True
        if isinstance(other, dict):
            if len(self) != len(other):
                return False
            for key in self:
                if self[key] != other.get(key, _MISSING):
                    return False
            for key in other:
                if key not in self:
                    return False
            return True
        return NotImplemented

    def __ne__(self, other: Any) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    def __add__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        return Counter._from_handle(_MOLT_COUNTER_ADD(self._handle, other._handle))

    def __sub__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        return Counter._from_handle(_MOLT_COUNTER_SUB(self._handle, other._handle))

    def __or__(self, other) -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        return Counter._from_handle(_MOLT_COUNTER_OR(self._handle, other._handle))

    def __and__(self, other: "Counter") -> "Counter":
        if not isinstance(other, Counter):
            return NotImplemented
        return Counter._from_handle(_MOLT_COUNTER_AND(self._handle, other._handle))

    def __iadd__(self, other: "Counter"):
        if not isinstance(other, Counter):
            return NotImplemented
        new_handle = _MOLT_COUNTER_ADD(self._handle, other._handle)
        old_handle = self._handle
        self._handle = new_handle
        try:
            _MOLT_COUNTER_DROP(old_handle)
        except Exception:
            pass
        return self

    def __isub__(self, other: "Counter"):
        if not isinstance(other, Counter):
            return NotImplemented
        new_handle = _MOLT_COUNTER_SUB(self._handle, other._handle)
        old_handle = self._handle
        self._handle = new_handle
        try:
            _MOLT_COUNTER_DROP(old_handle)
        except Exception:
            pass
        return self

    def __ior__(self, other):
        if not isinstance(other, Counter):
            return NotImplemented
        new_handle = _MOLT_COUNTER_OR(self._handle, other._handle)
        old_handle = self._handle
        self._handle = new_handle
        try:
            _MOLT_COUNTER_DROP(old_handle)
        except Exception:
            pass
        return self

    def __iand__(self, other: "Counter"):
        if not isinstance(other, Counter):
            return NotImplemented
        new_handle = _MOLT_COUNTER_AND(self._handle, other._handle)
        old_handle = self._handle
        self._handle = new_handle
        try:
            _MOLT_COUNTER_DROP(old_handle)
        except Exception:
            pass
        return self

    def __hash__(self):
        raise TypeError("unhashable type: 'Counter'")

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_COUNTER_DROP(handle)
            except Exception:
                pass


# ---------------------------------------------------------------------------
# defaultdict — subclasses dict, uses intrinsic for factory/__missing__
# ---------------------------------------------------------------------------


class defaultdict(dict):
    __slots__ = ("_dd_handle",)

    def __init__(self, default_factory=None, *args: Any, **kwargs: Any) -> None:
        self._dd_handle = _MOLT_DEFAULTDICT_NEW(default_factory)
        if len(args) > 1:
            raise TypeError("defaultdict expected at most 1 positional argument")
        if args:
            dict.update(self, args[0])
        if kwargs:
            dict.update(self, kwargs)

    @property
    def default_factory(self):
        return _MOLT_DEFAULTDICT_FACTORY(self._dd_handle)

    @default_factory.setter
    def default_factory(self, value):
        old_handle = self._dd_handle
        self._dd_handle = _MOLT_DEFAULTDICT_NEW(value)
        try:
            _MOLT_DEFAULTDICT_DROP(old_handle)
        except Exception:
            pass

    def __getitem__(self, key: Any) -> Any:
        try:
            return dict.__getitem__(self, key)
        except KeyError:
            return self.__missing__(key)

    def __missing__(self, key: Any) -> Any:
        result = _MOLT_DEFAULTDICT_MISSING(self._dd_handle, key)
        dict.__setitem__(self, key, result)
        return result

    def copy(self) -> "defaultdict":
        factory = self.default_factory
        new_dd = defaultdict(factory)
        dict.update(new_dd, self)
        return new_dd

    def __repr__(self) -> str:
        return f"defaultdict({self.default_factory!r}, {dict(self)!r})"

    def __del__(self):
        handle = getattr(self, "_dd_handle", None)
        if handle is not None:
            try:
                _MOLT_DEFAULTDICT_DROP(handle)
            except Exception:
                pass


# ---------------------------------------------------------------------------
# ChainMap — intrinsic-backed (handle-based)
# ---------------------------------------------------------------------------
# NOTE: isinstance(chain_map, dict) is False. ChainMap uses handle-based
# storage delegated to Rust intrinsics. The first map in `maps` is the
# primary map; writes and deletes go there only.
# ---------------------------------------------------------------------------


class _ChainMapIter:
    """Forward iterator over unique keys of an intrinsic-backed ChainMap."""

    __slots__ = ("_keys", "_index")

    def __init__(self, chain_map: "ChainMap") -> None:
        self._keys = _MOLT_CHAINMAP_KEYS(chain_map._handle)
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self) -> Any:
        if self._index >= len(self._keys):
            raise StopIteration
        key = self._keys[self._index]
        self._index += 1
        return key


class ChainMap:
    __slots__ = ("_handle",)

    def __init__(self, *maps) -> None:
        if maps:
            # Validate that all positional args are dicts.
            for m in maps:
                if not isinstance(m, dict):
                    raise TypeError("ChainMap maps must be dicts")
            self._handle = _MOLT_CHAINMAP_NEW(list(maps))
        else:
            # Empty ChainMap: pass None so the intrinsic allocates a fresh
            # empty primary dict.
            self._handle = _MOLT_CHAINMAP_NEW(None)

    @classmethod
    def _from_handle(cls, handle) -> "ChainMap":
        inst = cls.__new__(cls)
        inst._handle = handle
        return inst

    # --- Mapping protocol ---

    def __getitem__(self, key: Any) -> Any:
        return _MOLT_CHAINMAP_GETITEM(self._handle, key)

    def __setitem__(self, key: Any, value: Any) -> None:
        _MOLT_CHAINMAP_SETITEM(self._handle, key, value)

    def __delitem__(self, key: Any) -> None:
        _MOLT_CHAINMAP_DELITEM(self._handle, key)

    def __contains__(self, key: Any) -> bool:
        return bool(_MOLT_CHAINMAP_CONTAINS(self._handle, key))

    def __len__(self) -> int:
        return int(_MOLT_CHAINMAP_LEN(self._handle))

    def __bool__(self) -> bool:
        return len(self) > 0

    def __iter__(self):
        return _ChainMapIter(self)

    # --- Views ---

    def keys(self):
        return list(_MOLT_CHAINMAP_KEYS(self._handle))

    def values(self) -> list:
        ks = _MOLT_CHAINMAP_KEYS(self._handle)
        return [_MOLT_CHAINMAP_GETITEM(self._handle, k) for k in ks]

    def items(self) -> list:
        ks = _MOLT_CHAINMAP_KEYS(self._handle)
        return [(k, _MOLT_CHAINMAP_GETITEM(self._handle, k)) for k in ks]

    def get(self, key: Any, default: Any = None) -> Any:
        if _MOLT_CHAINMAP_CONTAINS(self._handle, key):
            return _MOLT_CHAINMAP_GETITEM(self._handle, key)
        return default

    # --- ChainMap-specific API ---

    def new_child(self, m: dict | None = None) -> "ChainMap":
        """Return a new ChainMap with an optional map prepended."""
        if m is not None and not isinstance(m, dict):
            raise TypeError("new_child map must be a dict")
        new_handle = _MOLT_CHAINMAP_NEW_CHILD(self._handle, m)
        return ChainMap._from_handle(new_handle)

    @property
    def parents(self) -> "ChainMap":
        """Return a new ChainMap containing all maps except the first."""
        new_handle = _MOLT_CHAINMAP_PARENTS(self._handle)
        return ChainMap._from_handle(new_handle)

    @property
    def maps(self) -> list:
        """Return the list of underlying dict objects."""
        return list(_MOLT_CHAINMAP_MAPS(self._handle))

    # --- Repr ---

    def __repr__(self) -> str:
        maps = _MOLT_CHAINMAP_MAPS(self._handle)
        return f"ChainMap({', '.join(repr(m) for m in maps)})"

    # --- Equality ---

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, ChainMap):
            if len(self) != len(other):
                return False
            for key in self:
                if key not in other or self[key] != other[key]:
                    return False
            return True
        if isinstance(other, dict):
            if len(self) != len(other):
                return False
            for key in self:
                if key not in other or self[key] != other[key]:
                    return False
            return True
        return NotImplemented

    def __ne__(self, other: Any) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    def __hash__(self):
        raise TypeError("unhashable type: 'ChainMap'")

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_CHAINMAP_DROP(handle)
            except Exception:
                pass
