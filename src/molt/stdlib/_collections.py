# Shim churn audit: 0 intrinsic-direct / 1 total exports (class-based, no pure-forwarding shims)
"""Intrinsic-backed ``_collections`` helpers for Molt.

The module mirrors CPython's public `_collections` surface:
`deque`, `defaultdict`, and `OrderedDict`.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["OrderedDict"]

_MOLT_ORDEREDDICT_NEW = _require_intrinsic("molt_ordereddict_new")
_MOLT_ORDEREDDICT_FROM_PAIRS = _require_intrinsic("molt_ordereddict_from_pairs")
_MOLT_ORDEREDDICT_SETITEM = _require_intrinsic("molt_ordereddict_setitem")
_MOLT_ORDEREDDICT_GETITEM = _require_intrinsic("molt_ordereddict_getitem")
_MOLT_ORDEREDDICT_DELITEM = _require_intrinsic("molt_ordereddict_delitem")
_MOLT_ORDEREDDICT_CONTAINS = _require_intrinsic("molt_ordereddict_contains")
_MOLT_ORDEREDDICT_LEN = _require_intrinsic("molt_ordereddict_len")
_MOLT_ORDEREDDICT_KEYS = _require_intrinsic("molt_ordereddict_keys")
_MOLT_ORDEREDDICT_VALUES = _require_intrinsic("molt_ordereddict_values")
_MOLT_ORDEREDDICT_ITEMS = _require_intrinsic("molt_ordereddict_items")
_MOLT_ORDEREDDICT_MOVE_TO_END = _require_intrinsic("molt_ordereddict_move_to_end")
_MOLT_ORDEREDDICT_POP = _require_intrinsic("molt_ordereddict_pop")
_MOLT_ORDEREDDICT_POPITEM = _require_intrinsic("molt_ordereddict_popitem")
_MOLT_ORDEREDDICT_UPDATE = _require_intrinsic("molt_ordereddict_update")
_MOLT_ORDEREDDICT_CLEAR = _require_intrinsic("molt_ordereddict_clear")
_MOLT_ORDEREDDICT_COPY = _require_intrinsic("molt_ordereddict_copy")
_MOLT_ORDEREDDICT_DROP = _require_intrinsic("molt_ordereddict_drop")

_MISSING = object()


class OrderedDict:
    __slots__ = ("_handle",)

    def __init__(self, *args, **kwargs) -> None:
        if len(args) > 1:
            raise TypeError(f"expected at most 1 arguments, got {len(args)}")
        if args and not kwargs and isinstance(args[0], (list, tuple)):
            self._handle = _MOLT_ORDEREDDICT_FROM_PAIRS(args[0])
            return
        self._handle = _MOLT_ORDEREDDICT_NEW()
        if args or kwargs:
            self.update(*args, **kwargs)

    @classmethod
    def _from_handle(cls, handle: int) -> "OrderedDict":
        inst = cls.__new__(cls)
        inst._handle = int(handle)
        return inst

    def __len__(self) -> int:
        return int(_MOLT_ORDEREDDICT_LEN(self._handle))

    def __iter__(self):
        return iter(_MOLT_ORDEREDDICT_KEYS(self._handle))

    def __reversed__(self):
        return reversed(_MOLT_ORDEREDDICT_KEYS(self._handle))

    def __contains__(self, key) -> bool:
        return bool(_MOLT_ORDEREDDICT_CONTAINS(self._handle, key))

    def __getitem__(self, key):
        return _MOLT_ORDEREDDICT_GETITEM(self._handle, key)

    def __setitem__(self, key, value) -> None:
        _MOLT_ORDEREDDICT_SETITEM(self._handle, key, value)

    def __delitem__(self, key) -> None:
        _MOLT_ORDEREDDICT_DELITEM(self._handle, key)

    def __repr__(self) -> str:
        cls_name = type(self).__name__
        if not len(self):
            return f"{cls_name}()"
        return f"{cls_name}({list(self.items())!r})"

    def __eq__(self, other):
        if isinstance(other, OrderedDict):
            return list(self.items()) == list(other.items())
        if hasattr(other, "items"):
            try:
                return dict(self.items()) == dict(other.items())
            except Exception:
                return NotImplemented
        return NotImplemented

    def __or__(self, other):
        if not hasattr(other, "items"):
            return NotImplemented
        result = self.copy()
        result.update(other)
        return result

    def __ror__(self, other):
        if not hasattr(other, "items"):
            return NotImplemented
        result = type(self)()
        result.update(other)
        result.update(self)
        return result

    def __ior__(self, other):
        self.update(other)
        return self

    def keys(self):
        return _MOLT_ORDEREDDICT_KEYS(self._handle)

    def values(self):
        return _MOLT_ORDEREDDICT_VALUES(self._handle)

    def items(self):
        return _MOLT_ORDEREDDICT_ITEMS(self._handle)

    def get(self, key, default=None):
        try:
            return _MOLT_ORDEREDDICT_GETITEM(self._handle, key)
        except KeyError:
            return default

    def clear(self) -> None:
        _MOLT_ORDEREDDICT_CLEAR(self._handle)

    def copy(self) -> "OrderedDict":
        return type(self)._from_handle(_MOLT_ORDEREDDICT_COPY(self._handle))

    @classmethod
    def fromkeys(cls, iterable, value=None):
        out = cls()
        for key in iterable:
            out[key] = value
        return out

    def move_to_end(self, key, last: bool = True) -> None:
        _MOLT_ORDEREDDICT_MOVE_TO_END(self._handle, key, bool(last))

    def popitem(self, last: bool = True):
        return _MOLT_ORDEREDDICT_POPITEM(self._handle, bool(last))

    def pop(self, key, default=_MISSING):
        return _MOLT_ORDEREDDICT_POP(self._handle, key, default)

    def setdefault(self, key, default=None):
        try:
            return _MOLT_ORDEREDDICT_GETITEM(self._handle, key)
        except KeyError:
            _MOLT_ORDEREDDICT_SETITEM(self._handle, key, default)
            return default

    def update(self, *args, **kwargs) -> None:
        if len(args) > 1:
            raise TypeError(f"expected at most 1 arguments, got {len(args)}")
        if args:
            other = args[0]
            if isinstance(other, OrderedDict):
                _MOLT_ORDEREDDICT_UPDATE(self._handle, other._handle)
            elif isinstance(other, dict):
                _MOLT_ORDEREDDICT_UPDATE(self._handle, other)
            elif hasattr(other, "keys"):
                for key in other.keys():
                    _MOLT_ORDEREDDICT_SETITEM(self._handle, key, other[key])
            else:
                for key, value in other:
                    _MOLT_ORDEREDDICT_SETITEM(self._handle, key, value)
        for key, value in kwargs.items():
            _MOLT_ORDEREDDICT_SETITEM(self._handle, key, value)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_ORDEREDDICT_DROP(handle)
            except Exception:
                pass
