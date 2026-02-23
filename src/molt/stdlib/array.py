"""Intrinsic-backed array for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ARRAY_NEW = _require_intrinsic("molt_array_new", globals())
_MOLT_ARRAY_FROM_LIST = _require_intrinsic("molt_array_from_list", globals())
_MOLT_ARRAY_APPEND = _require_intrinsic("molt_array_append", globals())
_MOLT_ARRAY_BUFFER_INFO = _require_intrinsic("molt_array_buffer_info", globals())
_MOLT_ARRAY_COUNT = _require_intrinsic("molt_array_count", globals())
_MOLT_ARRAY_DROP = _require_intrinsic("molt_array_drop", globals())
_MOLT_ARRAY_EXTEND = _require_intrinsic("molt_array_extend", globals())
_MOLT_ARRAY_FROMBYTES = _require_intrinsic("molt_array_frombytes", globals())
_MOLT_ARRAY_GETITEM = _require_intrinsic("molt_array_getitem", globals())
_MOLT_ARRAY_INDEX = _require_intrinsic("molt_array_index", globals())
_MOLT_ARRAY_INSERT = _require_intrinsic("molt_array_insert", globals())
_MOLT_ARRAY_ITEMSIZE = _require_intrinsic("molt_array_itemsize", globals())
_MOLT_ARRAY_LEN = _require_intrinsic("molt_array_len", globals())
_MOLT_ARRAY_POP = _require_intrinsic("molt_array_pop", globals())
_MOLT_ARRAY_REMOVE = _require_intrinsic("molt_array_remove", globals())
_MOLT_ARRAY_REVERSE = _require_intrinsic("molt_array_reverse", globals())
_MOLT_ARRAY_SETITEM = _require_intrinsic("molt_array_setitem", globals())
_MOLT_ARRAY_TOBYTES = _require_intrinsic("molt_array_tobytes", globals())
_MOLT_ARRAY_TOLIST = _require_intrinsic("molt_array_tolist", globals())
_MOLT_ARRAY_TYPECODE = _require_intrinsic("molt_array_typecode", globals())

__all__ = ["array", "typecodes", "ArrayType"]

typecodes: str = "bBuhHiIlLqQfd"


class array:
    """array(typecode [, initializer]) -> array

    Return a new array whose items are restricted by typecode, and
    initialized from the optional initializer value, which must be a
    list, a bytes-like object, or iterable over elements of the
    appropriate type.
    """

    def __init__(self, typecode: str, initializer=None) -> None:
        if not isinstance(typecode, str) or len(typecode) != 1:
            raise TypeError(
                "array() argument 1 must be a unicode character, not "
                + type(typecode).__name__
            )
        if typecode not in typecodes:
            raise ValueError(
                "bad typecode (must be b, B, u, h, H, i, I, l, L, q, Q, f, or d)"
            )
        if initializer is not None:
            if isinstance(initializer, (list, tuple)):
                self._handle = _MOLT_ARRAY_FROM_LIST(typecode, list(initializer))
            elif isinstance(initializer, (bytes, bytearray)):
                self._handle = _MOLT_ARRAY_NEW(typecode)
                _MOLT_ARRAY_FROMBYTES(self._handle, initializer)
            else:
                self._handle = _MOLT_ARRAY_FROM_LIST(typecode, list(initializer))
        else:
            self._handle = _MOLT_ARRAY_NEW(typecode)

    @property
    def typecode(self) -> str:
        """The typecode character used to create the array."""
        return _MOLT_ARRAY_TYPECODE(self._handle)

    @property
    def itemsize(self) -> int:
        """The length in bytes of one array item."""
        return _MOLT_ARRAY_ITEMSIZE(self._handle)

    def append(self, v) -> None:
        """Append new value v to the end of the array."""
        _MOLT_ARRAY_APPEND(self._handle, v)

    def buffer_info(self) -> tuple:
        """Return a tuple (address, length) giving the current memory address
        and the length in elements of the buffer."""
        return _MOLT_ARRAY_BUFFER_INFO(self._handle)

    def count(self, v) -> int:
        """Return number of occurrences of v in the array."""
        return _MOLT_ARRAY_COUNT(self._handle, v)

    def extend(self, items) -> None:
        """Append items to the end of the array."""
        if isinstance(items, array):
            _MOLT_ARRAY_EXTEND(self._handle, items.tolist())
        elif isinstance(items, (list, tuple)):
            _MOLT_ARRAY_EXTEND(self._handle, list(items))
        else:
            _MOLT_ARRAY_EXTEND(self._handle, list(items))

    def frombytes(self, data: bytes) -> None:
        """Appends items from the bytes object."""
        _MOLT_ARRAY_FROMBYTES(self._handle, data)

    def index(self, v) -> int:
        """Return index of first occurrence of v in the array."""
        return _MOLT_ARRAY_INDEX(self._handle, v)

    def insert(self, i: int, v) -> None:
        """Insert a new item v into the array before position i."""
        _MOLT_ARRAY_INSERT(self._handle, i, v)

    def pop(self, i: int = -1):
        """Remove the item with the index i from the array and return it."""
        return _MOLT_ARRAY_POP(self._handle, i)

    def remove(self, v) -> None:
        """Remove the first occurrence of v in the array."""
        _MOLT_ARRAY_REMOVE(self._handle, v)

    def reverse(self) -> None:
        """Reverse the order of the items in the array."""
        _MOLT_ARRAY_REVERSE(self._handle)

    def tobytes(self) -> bytes:
        """Convert the array to an array of machine values and return the
        bytes representation."""
        return _MOLT_ARRAY_TOBYTES(self._handle)

    def tolist(self) -> list:
        """Convert array to an ordinary list with the same items."""
        return _MOLT_ARRAY_TOLIST(self._handle)

    def __len__(self) -> int:
        return _MOLT_ARRAY_LEN(self._handle)

    def __getitem__(self, index: int):
        return _MOLT_ARRAY_GETITEM(self._handle, index)

    def __setitem__(self, index: int, value) -> None:
        _MOLT_ARRAY_SETITEM(self._handle, index, value)

    def __repr__(self) -> str:
        tc = self.typecode
        items = self.tolist()
        if tc == "u":
            return f"array('{tc}', {items!r})"
        return f"array('{tc}', {items!r})"

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            _MOLT_ARRAY_DROP(handle)


ArrayType = array
