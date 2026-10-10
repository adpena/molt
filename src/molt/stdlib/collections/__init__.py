"""Collections helpers for Molt (intrinsic-backed)."""

from __future__ import annotations

import sys as _sys
from _compatibility_errors import counter_fromkeys_error as _counter_fromkeys_error

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
    "UserDict",
    "UserList",
    "UserString",
]

_MISSING = object()

# --- Class-building intrinsics (for namedtuple) ---
_MOLT_CLASS_NEW = _require_intrinsic("molt_class_new")
_MOLT_CLASS_SET_BASE = _require_intrinsic("molt_class_set_base")
_MOLT_CLASS_APPLY_SET_NAME = _require_intrinsic("molt_class_apply_set_name")
_MOLT_NAMEDTUPLE_VALIDATE = _require_intrinsic("molt_namedtuple_validate_fields")

# --- deque intrinsics ---
_MOLT_DEQUE_NEW = _require_intrinsic("molt_deque_new")
_MOLT_DEQUE_FROM_ITERABLE = _require_intrinsic("molt_deque_from_iterable")
_MOLT_DEQUE_APPEND = _require_intrinsic("molt_deque_append")
_MOLT_DEQUE_APPENDLEFT = _require_intrinsic("molt_deque_appendleft")
_MOLT_DEQUE_CLEAR = _require_intrinsic("molt_deque_clear")
_MOLT_DEQUE_CONTAINS = _require_intrinsic("molt_deque_contains")
_MOLT_DEQUE_COPY = _require_intrinsic("molt_deque_copy")
_MOLT_DEQUE_COUNT = _require_intrinsic("molt_deque_count")
_MOLT_DEQUE_DELITEM = _require_intrinsic("molt_deque_delitem")
_MOLT_DEQUE_DROP = _require_intrinsic("molt_deque_drop")
_MOLT_DEQUE_EXTEND = _require_intrinsic("molt_deque_extend")
_MOLT_DEQUE_EXTENDLEFT = _require_intrinsic("molt_deque_extendleft")
_MOLT_DEQUE_GETITEM = _require_intrinsic("molt_deque_getitem")
_MOLT_DEQUE_INDEX = _require_intrinsic("molt_deque_index")
_MOLT_DEQUE_INSERT = _require_intrinsic("molt_deque_insert")
_MOLT_DEQUE_LEN = _require_intrinsic("molt_deque_len")
_MOLT_DEQUE_MAXLEN = _require_intrinsic("molt_deque_maxlen")
_MOLT_DEQUE_POP = _require_intrinsic("molt_deque_pop")
_MOLT_DEQUE_POPLEFT = _require_intrinsic("molt_deque_popleft")
_MOLT_DEQUE_REMOVE = _require_intrinsic("molt_deque_remove")
_MOLT_DEQUE_REVERSE = _require_intrinsic("molt_deque_reverse")
_MOLT_DEQUE_ROTATE = _require_intrinsic("molt_deque_rotate")
_MOLT_DEQUE_SETITEM = _require_intrinsic("molt_deque_setitem")

# --- defaultdict intrinsics ---
_MOLT_DEFAULTDICT_DROP = _require_intrinsic("molt_defaultdict_drop")
_MOLT_DEFAULTDICT_FACTORY = _require_intrinsic("molt_defaultdict_factory")
_MOLT_DEFAULTDICT_MISSING_METHOD = _require_intrinsic("molt_defaultdict_missing_method")
_MOLT_DEFAULTDICT_NEW = _require_intrinsic("molt_defaultdict_new")

# --- ChainMap intrinsics ---
_MOLT_CHAINMAP_CONTAINS = _require_intrinsic("molt_chainmap_contains")
_MOLT_CHAINMAP_DELITEM = _require_intrinsic("molt_chainmap_delitem")
_MOLT_CHAINMAP_DROP = _require_intrinsic("molt_chainmap_drop")
_MOLT_CHAINMAP_GETITEM = _require_intrinsic("molt_chainmap_getitem")
_MOLT_CHAINMAP_KEYS = _require_intrinsic("molt_chainmap_keys")
_MOLT_CHAINMAP_LEN = _require_intrinsic("molt_chainmap_len")
_MOLT_CHAINMAP_MAPS = _require_intrinsic("molt_chainmap_maps")
_MOLT_CHAINMAP_NEW = _require_intrinsic("molt_chainmap_new")
_MOLT_CHAINMAP_NEW_CHILD = _require_intrinsic("molt_chainmap_new_child")
_MOLT_CHAINMAP_PARENTS = _require_intrinsic("molt_chainmap_parents")
_MOLT_CHAINMAP_SETITEM = _require_intrinsic("molt_chainmap_setitem")


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

    # Validate typename and field names in Rust — raises ValueError on invalid.
    field_names = list(_MOLT_NAMEDTUPLE_VALIDATE(typename, field_names, rename))
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


# Counter follows CPython 3.12.13 Lib/collections/__init__.py.
# Copyright (c) 2001-2026 Python Software Foundation; PSF-2.0.
# See LICENSE.cpython. Molt compiles these methods and their dependencies;
# storage and dispatch use the ordinary native dict subclass authorities.
from itertools import chain as _chain, repeat as _repeat, starmap as _starmap
from operator import itemgetter as _itemgetter
from _collections import _count_elements


class Counter(dict):
    '''Dict subclass for counting hashable items.  Sometimes called a bag
    or multiset.  Elements are stored as dictionary keys and their counts
    are stored as dictionary values.

    >>> c = Counter('abcdeabcdabcaba')  # count elements from a string

    >>> c.most_common(3)                # three most common elements
    [('a', 5), ('b', 4), ('c', 3)]
    >>> sorted(c)                       # list all unique elements
    ['a', 'b', 'c', 'd', 'e']
    >>> ''.join(sorted(c.elements()))   # list elements with repetitions
    'aaaaabbbbcccdde'
    >>> sum(c.values())                 # total of all counts
    15

    >>> c['a']                          # count of letter 'a'
    5
    >>> for elem in 'shazam':           # update counts from an iterable
    ...     c[elem] += 1                # by adding 1 to each element's count
    >>> c['a']                          # now there are seven 'a'
    7
    >>> del c['b']                      # remove all 'b'
    >>> c['b']                          # now there are zero 'b'
    0

    >>> d = Counter('simsalabim')       # make another counter
    >>> c.update(d)                     # add in the second counter
    >>> c['a']                          # now there are nine 'a'
    9

    >>> c.clear()                       # empty the counter
    >>> c
    Counter()

    Note:  If a count is set to zero or reduced to zero, it will remain
    in the counter until the entry is deleted or the counter is cleared:

    >>> c = Counter('aaabbc')
    >>> c['b'] -= 2                     # reduce the count of 'b' by two
    >>> c.most_common()                 # 'b' is still in, but its count is zero
    [('a', 3), ('c', 1), ('b', 0)]

    '''
    # References:
    #   http://en.wikipedia.org/wiki/Multiset
    #   http://www.gnu.org/software/smalltalk/manual-base/html_node/Bag.html
    #   http://www.java2s.com/Tutorial/Cpp/0380__set-multiset/Catalog0380__set-multiset.htm
    #   http://code.activestate.com/recipes/259174/
    #   Knuth, TAOCP Vol. II section 4.6.3

    def __init__(self, iterable=None, /, **kwds):
        '''Create a new, empty Counter object.  And if given, count elements
        from an input iterable.  Or, initialize the count from another mapping
        of elements to their counts.

        >>> c = Counter()                           # a new, empty counter
        >>> c = Counter('gallahad')                 # a new counter from an iterable
        >>> c = Counter({'a': 4, 'b': 2})           # a new counter from a mapping
        >>> c = Counter(a=4, b=2)                   # a new counter from keyword args

        '''
        super().__init__()
        self.update(iterable, **kwds)

    def __missing__(self, key):
        'The count of elements not in the Counter is zero.'
        # Needed so that self[missing_item] does not raise KeyError
        return 0

    def total(self):
        'Sum of the counts'
        return sum(self.values())

    def most_common(self, n=None):
        '''List the n most common elements and their counts from the most
        common to the least.  If n is None, then list all element counts.

        >>> Counter('abracadabra').most_common(3)
        [('a', 5), ('b', 2), ('r', 2)]

        '''
        # Emulate Bag.sortedByCount from Smalltalk
        if n is None:
            return sorted(self.items(), key=_itemgetter(1), reverse=True)

        # Lazy import to speedup Python startup time
        import heapq
        return heapq.nlargest(n, self.items(), key=_itemgetter(1))

    def elements(self):
        '''Iterator over elements repeating each as many times as its count.

        >>> c = Counter('ABCABC')
        >>> sorted(c.elements())
        ['A', 'A', 'B', 'B', 'C', 'C']

        Knuth's example for prime factors of 1836:  2**2 * 3**3 * 17**1

        >>> import math
        >>> prime_factors = Counter({2: 2, 3: 3, 17: 1})
        >>> math.prod(prime_factors.elements())
        1836

        Note, if an element's count has been set to zero or is a negative
        number, elements() will ignore it.

        '''
        # Emulate Bag.do from Smalltalk and Multiset.begin from C++.
        return _chain.from_iterable(_starmap(_repeat, self.items()))

    # Override dict methods where necessary

    @classmethod
    def fromkeys(cls, iterable, v=None):
        # There is no equivalent method for counters because the semantics
        # would be ambiguous in cases such as Counter.fromkeys('aaabbc', v=2).
        # Initializing counters to zero values isn't necessary because zero
        # is already the default value for counter lookups.  Initializing
        # to one is easily accomplished with Counter(set(iterable)).  For
        # more exotic cases, create a dictionary first using a dictionary
        # comprehension or dict.fromkeys().
        raise _counter_fromkeys_error()

    def update(self, iterable=None, /, **kwds):
        '''Like dict.update() but add counts instead of replacing them.

        Source can be an iterable, a dictionary, or another Counter instance.

        >>> c = Counter('which')
        >>> c.update('witch')           # add elements from another iterable
        >>> d = Counter('watch')
        >>> c.update(d)                 # add elements from another counter
        >>> c['h']                      # four 'h' in which, witch, and watch
        4

        '''
        # The regular dict.update() operation makes no sense here because the
        # replace behavior results in some of the original untouched counts
        # being mixed-in with all of the other counts for a mismash that
        # doesn't have a straight-forward interpretation in most counting
        # contexts.  Instead, we implement straight-addition.  Both the inputs
        # and outputs are allowed to contain zero and negative counts.

        if iterable is not None:
            if isinstance(iterable, abc.Mapping):
                if self:
                    self_get = self.get
                    for elem, count in iterable.items():
                        self[elem] = count + self_get(elem, 0)
                else:
                    # fast path when counter is empty
                    super().update(iterable)
            else:
                _count_elements(self, iterable)
        if kwds:
            self.update(kwds)

    def subtract(self, iterable=None, /, **kwds):
        '''Like dict.update() but subtracts counts instead of replacing them.
        Counts can be reduced below zero.  Both the inputs and outputs are
        allowed to contain zero and negative counts.

        Source can be an iterable, a dictionary, or another Counter instance.

        >>> c = Counter('which')
        >>> c.subtract('witch')             # subtract elements from another iterable
        >>> c.subtract(Counter('watch'))    # subtract elements from another counter
        >>> c['h']                          # 2 in which, minus 1 in witch, minus 1 in watch
        0
        >>> c['w']                          # 1 in which, minus 1 in witch, minus 1 in watch
        -1

        '''
        if iterable is not None:
            self_get = self.get
            if isinstance(iterable, abc.Mapping):
                for elem, count in iterable.items():
                    self[elem] = self_get(elem, 0) - count
            else:
                for elem in iterable:
                    self[elem] = self_get(elem, 0) - 1
        if kwds:
            self.subtract(kwds)

    def copy(self):
        'Return a shallow copy.'
        return self.__class__(self)

    def __reduce__(self):
        return self.__class__, (dict(self),)

    def __delitem__(self, elem):
        'Like dict.__delitem__() but does not raise KeyError for missing values.'
        if elem in self:
            super().__delitem__(elem)

    def __repr__(self):
        if not self:
            return f'{self.__class__.__name__}()'
        try:
            # dict() preserves the ordering returned by most_common()
            d = dict(self.most_common())
        except TypeError:
            # handle case where values are not orderable
            d = dict(self)
        return f'{self.__class__.__name__}({d!r})'

    # Multiset-style mathematical operations discussed in:
    #       Knuth TAOCP Volume II section 4.6.3 exercise 19
    #       and at http://en.wikipedia.org/wiki/Multiset
    #
    # Outputs guaranteed to only include positive counts.
    #
    # To strip negative and zero counts, add-in an empty counter:
    #       c += Counter()
    #
    # Results are ordered according to when an element is first
    # encountered in the left operand and then by the order
    # encountered in the right operand.
    #
    # When the multiplicities are all zero or one, multiset operations
    # are guaranteed to be equivalent to the corresponding operations
    # for regular sets.
    #     Given counter multisets such as:
    #         cp = Counter(a=1, b=0, c=1)
    #         cq = Counter(c=1, d=0, e=1)
    #     The corresponding regular sets would be:
    #         sp = {'a', 'c'}
    #         sq = {'c', 'e'}
    #     All of the following relations would hold:
    #         set(cp + cq) == sp | sq
    #         set(cp - cq) == sp - sq
    #         set(cp | cq) == sp | sq
    #         set(cp & cq) == sp & sq
    #         (cp == cq) == (sp == sq)
    #         (cp != cq) == (sp != sq)
    #         (cp <= cq) == (sp <= sq)
    #         (cp < cq) == (sp < sq)
    #         (cp >= cq) == (sp >= sq)
    #         (cp > cq) == (sp > sq)

    def __eq__(self, other):
        'True if all counts agree. Missing counts are treated as zero.'
        if not isinstance(other, Counter):
            return NotImplemented
        return all(self[e] == other[e] for c in (self, other) for e in c)

    def __ne__(self, other):
        'True if any counts disagree. Missing counts are treated as zero.'
        if not isinstance(other, Counter):
            return NotImplemented
        return not self == other

    def __le__(self, other):
        'True if all counts in self are a subset of those in other.'
        if not isinstance(other, Counter):
            return NotImplemented
        return all(self[e] <= other[e] for c in (self, other) for e in c)

    def __lt__(self, other):
        'True if all counts in self are a proper subset of those in other.'
        if not isinstance(other, Counter):
            return NotImplemented
        return self <= other and self != other

    def __ge__(self, other):
        'True if all counts in self are a superset of those in other.'
        if not isinstance(other, Counter):
            return NotImplemented
        return all(self[e] >= other[e] for c in (self, other) for e in c)

    def __gt__(self, other):
        'True if all counts in self are a proper superset of those in other.'
        if not isinstance(other, Counter):
            return NotImplemented
        return self >= other and self != other

    def __add__(self, other):
        '''Add counts from two counters.

        >>> Counter('abbb') + Counter('bcc')
        Counter({'b': 4, 'c': 2, 'a': 1})

        '''
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter()
        for elem, count in self.items():
            newcount = count + other[elem]
            if newcount > 0:
                result[elem] = newcount
        for elem, count in other.items():
            if elem not in self and count > 0:
                result[elem] = count
        return result

    def __sub__(self, other):
        ''' Subtract count, but keep only results with positive counts.

        >>> Counter('abbbc') - Counter('bccd')
        Counter({'b': 2, 'a': 1})

        '''
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter()
        for elem, count in self.items():
            newcount = count - other[elem]
            if newcount > 0:
                result[elem] = newcount
        for elem, count in other.items():
            if elem not in self and count < 0:
                result[elem] = 0 - count
        return result

    def __or__(self, other):
        '''Union is the maximum of value in either of the input counters.

        >>> Counter('abbb') | Counter('bcc')
        Counter({'b': 3, 'c': 2, 'a': 1})

        '''
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter()
        for elem, count in self.items():
            other_count = other[elem]
            newcount = other_count if count < other_count else count
            if newcount > 0:
                result[elem] = newcount
        for elem, count in other.items():
            if elem not in self and count > 0:
                result[elem] = count
        return result

    def __and__(self, other):
        ''' Intersection is the minimum of corresponding counts.

        >>> Counter('abbb') & Counter('bcc')
        Counter({'b': 1})

        '''
        if not isinstance(other, Counter):
            return NotImplemented
        result = Counter()
        for elem, count in self.items():
            other_count = other[elem]
            newcount = count if count < other_count else other_count
            if newcount > 0:
                result[elem] = newcount
        return result

    def __pos__(self):
        'Adds an empty counter, effectively stripping negative and zero counts'
        result = Counter()
        for elem, count in self.items():
            if count > 0:
                result[elem] = count
        return result

    def __neg__(self):
        '''Subtracts from an empty counter.  Strips positive and zero counts,
        and flips the sign on negative counts.

        '''
        result = Counter()
        for elem, count in self.items():
            if count < 0:
                result[elem] = 0 - count
        return result

    def _keep_positive(self):
        '''Internal method to strip elements with a negative or zero count'''
        nonpositive = [elem for elem, count in self.items() if not count > 0]
        for elem in nonpositive:
            del self[elem]
        return self

    def __iadd__(self, other):
        '''Inplace add from another counter, keeping only positive counts.

        >>> c = Counter('abbb')
        >>> c += Counter('bcc')
        >>> c
        Counter({'b': 4, 'c': 2, 'a': 1})

        '''
        for elem, count in other.items():
            self[elem] += count
        return self._keep_positive()

    def __isub__(self, other):
        '''Inplace subtract counter, but keep only results with positive counts.

        >>> c = Counter('abbbc')
        >>> c -= Counter('bccd')
        >>> c
        Counter({'b': 2, 'a': 1})

        '''
        for elem, count in other.items():
            self[elem] -= count
        return self._keep_positive()

    def __ior__(self, other):
        '''Inplace union is the maximum of value from either counter.

        >>> c = Counter('abbb')
        >>> c |= Counter('bcc')
        >>> c
        Counter({'b': 3, 'c': 2, 'a': 1})

        '''
        for elem, other_count in other.items():
            count = self[elem]
            if other_count > count:
                self[elem] = other_count
        return self._keep_positive()

    def __iand__(self, other):
        '''Inplace intersection is the minimum of corresponding counts.

        >>> c = Counter('abbb')
        >>> c &= Counter('bcc')
        >>> c
        Counter({'b': 1})

        '''
        for elem, count in self.items():
            other_count = other[elem]
            if other_count < count:
                self[elem] = other_count
        return self._keep_positive()


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

    __missing__ = _MOLT_DEFAULTDICT_MISSING_METHOD

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


# ---------------------------------------------------------------------------
# UserDict — pure Python dict wrapper, designed for subclassing
# ---------------------------------------------------------------------------


class UserDict:
    """A plain-dict wrapper that is safe to subclass.

    Mirrors CPython's ``collections.UserDict`` (Python >= 3.12).
    The underlying storage is ``self.data``, a plain :class:`dict`.
    """

    # No __slots__: subclasses need to be able to add instance attributes.

    def __init__(self, dict=None, /, **kwargs) -> None:
        self.data: Any = {}
        if dict is not None:
            self.update(dict)
        if kwargs:
            self.update(kwargs)

    # --- Repr / equality ---

    def __repr__(self) -> str:
        return repr(self.data)

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, UserDict):
            return self.data == other.data
        return self.data == other

    def __ne__(self, other: Any) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    # --- Mapping protocol ---

    def __len__(self) -> int:
        return len(self.data)

    def __getitem__(self, key: Any) -> Any:
        if key in self.data:
            return self.data[key]
        if hasattr(self.__class__, "__missing__"):
            return self.__class__.__missing__(self, key)
        raise KeyError(key)

    def __setitem__(self, key: Any, item: Any) -> None:
        self.data[key] = item

    def __delitem__(self, key: Any) -> None:
        del self.data[key]

    def __iter__(self):
        return iter(self.data)

    def __contains__(self, key: Any) -> bool:
        return key in self.data

    # --- Dict views ---

    def keys(self):
        return self.data.keys()

    def items(self):
        return self.data.items()

    def values(self):
        return self.data.values()

    # --- Common dict methods ---

    def get(self, key: Any, default: Any = None) -> Any:
        return self.data.get(key, default)

    def pop(self, key: Any, *args) -> Any:
        return self.data.pop(key, *args)

    def popitem(self) -> tuple:
        return self.data.popitem()

    def clear(self) -> None:
        self.data.clear()

    def setdefault(self, key: Any, default: Any = None) -> Any:
        if key not in self.data:
            self.data[key] = default
        return self.data[key]

    def update(self, dict=None, /, **kwargs) -> None:  # type: ignore[override]
        if dict is not None:
            if isinstance(dict, UserDict):
                self.data.update(dict.data)
            elif hasattr(dict, "keys"):
                for key in dict.keys():
                    self[key] = dict[key]
            else:
                for key, value in dict:
                    self[key] = value
        for key, value in kwargs.items():
            self[key] = value

    # --- Merge operators (Python 3.9+) ---

    def __or__(self, other: Any) -> "UserDict":
        if isinstance(other, UserDict):
            new = self.__class__(self.data)
            new.update(other.data)
            return new
        if isinstance(other, dict):
            new = self.__class__(self.data)
            new.update(other)
            return new
        return NotImplemented

    def __ror__(self, other: Any) -> "UserDict":
        if isinstance(other, dict):
            new = self.__class__(other)
            new.update(self.data)
            return new
        return NotImplemented

    def __ior__(self, other: Any) -> "UserDict":
        if isinstance(other, UserDict):
            self.update(other.data)
        elif isinstance(other, dict):
            self.update(other)
        else:
            return NotImplemented
        return self

    # --- Copy ---

    def copy(self) -> "UserDict":
        if self.__class__ is UserDict:
            return UserDict(self.data.copy())
        import copy as _copy

        data = self.data
        try:
            self.data = {}
            c = _copy.copy(self)
        finally:
            self.data = data
        c.update(self)
        return c

    def __copy__(self) -> "UserDict":
        return self.copy()

    # --- Class method ---

    @classmethod
    def fromkeys(cls, iterable, value=None) -> "UserDict":
        d = cls()
        for key in iterable:
            d[key] = value
        return d

    def __hash__(self):  # type: ignore[override]
        raise TypeError("unhashable type: 'UserDict'")


# ---------------------------------------------------------------------------
# UserList — pure Python list wrapper, designed for subclassing
# ---------------------------------------------------------------------------


class UserList:
    """A plain-list wrapper that is safe to subclass.

    Mirrors CPython's ``collections.UserList`` (Python >= 3.12).
    The underlying storage is ``self.data``, a plain :class:`list`.
    """

    def __init__(self, initlist=None) -> None:
        self.data: Any = []
        if initlist is not None:
            if isinstance(initlist, list):
                self.data = initlist[:]
            elif isinstance(initlist, UserList):
                self.data = initlist.data[:]
            else:
                self.data = list(initlist)

    # --- Repr / equality / ordering ---

    def __repr__(self) -> str:
        return repr(self.data)

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, UserList):
            return self.data == other.data
        return self.data == other

    def __ne__(self, other: Any) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    def __lt__(self, other: Any) -> bool:
        if isinstance(other, UserList):
            return self.data < other.data
        return self.data < other

    def __le__(self, other: Any) -> bool:
        if isinstance(other, UserList):
            return self.data <= other.data
        return self.data <= other

    def __gt__(self, other: Any) -> bool:
        if isinstance(other, UserList):
            return self.data > other.data
        return self.data > other

    def __ge__(self, other: Any) -> bool:
        if isinstance(other, UserList):
            return self.data >= other.data
        return self.data >= other

    # --- Sequence protocol ---

    def __len__(self) -> int:
        return len(self.data)

    def __getitem__(self, i):
        if isinstance(i, slice):
            return self.__class__(self.data[i])
        return self.data[i]

    def __setitem__(self, i, item) -> None:
        self.data[i] = item

    def __delitem__(self, i) -> None:
        del self.data[i]

    def __iter__(self):
        return iter(self.data)

    def __contains__(self, item: Any) -> bool:
        return item in self.data

    def __reversed__(self):
        return reversed(self.data)

    # --- Concatenation / repetition ---

    def __add__(self, other: Any) -> "UserList":
        if isinstance(other, UserList):
            return self.__class__(self.data + other.data)
        if isinstance(other, list):
            return self.__class__(self.data + other)
        return self.__class__(self.data + list(other))

    def __radd__(self, other: Any) -> "UserList":
        if isinstance(other, UserList):
            return self.__class__(other.data + self.data)
        if isinstance(other, list):
            return self.__class__(other + self.data)
        return self.__class__(list(other) + self.data)

    def __iadd__(self, other: Any) -> "UserList":
        if isinstance(other, UserList):
            self.data += other.data
        else:
            self.data += list(other)
        return self

    def __mul__(self, n: int) -> "UserList":
        return self.__class__(self.data * n)

    def __rmul__(self, n: int) -> "UserList":
        return self.__class__(self.data * n)

    def __imul__(self, n: int) -> "UserList":
        self.data *= n
        return self

    # --- Mutable sequence methods ---

    def append(self, item: Any) -> None:
        self.data.append(item)

    def insert(self, i: int, item: Any) -> None:
        self.data.insert(i, item)

    def pop(self, i: int = -1) -> Any:
        return self.data.pop(i)

    def remove(self, item: Any) -> None:
        self.data.remove(item)

    def clear(self) -> None:
        self.data.clear()

    def copy(self) -> "UserList":
        return self.__class__(self.data.copy())

    def count(self, item: Any) -> int:
        return self.data.count(item)

    def index(self, item: Any, *args) -> int:
        return self.data.index(item, *args)

    def reverse(self) -> None:
        self.data.reverse()

    def sort(self, /, *args, **kwds) -> None:
        self.data.sort(*args, **kwds)

    def extend(self, other: Any) -> None:
        if isinstance(other, UserList):
            self.data.extend(other.data)
        else:
            self.data.extend(other)

    def __hash__(self):  # type: ignore[override]
        raise TypeError("unhashable type: 'UserList'")


# ---------------------------------------------------------------------------
# UserString — pure Python str wrapper, designed for subclassing
# ---------------------------------------------------------------------------


class UserString:
    """A plain-string wrapper that is safe to subclass.

    Mirrors CPython's ``collections.UserString`` (Python >= 3.12).
    The underlying storage is ``self.data``, a plain :class:`str`.
    """

    def __init__(self, seq: Any = "") -> None:
        if isinstance(seq, str):
            self.data = seq
        elif isinstance(seq, UserString):
            self.data = seq.data[:]
        else:
            self.data = str(seq)

    # --- Repr / str / bytes ---

    def __str__(self) -> str:
        return self.data

    def __repr__(self) -> str:
        return repr(self.data)

    def __bytes__(self) -> bytes:
        return self.data.encode()

    # --- Hashing / equality / ordering ---

    def __hash__(self) -> int:
        return hash(self.data)

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, UserString):
            return self.data == other.data
        return self.data == other

    def __ne__(self, other: Any) -> bool:
        result = self.__eq__(other)
        if result is NotImplemented:
            return NotImplemented
        return not result

    def __lt__(self, other: Any) -> bool:
        if isinstance(other, UserString):
            return self.data < other.data
        return self.data < other

    def __le__(self, other: Any) -> bool:
        if isinstance(other, UserString):
            return self.data <= other.data
        return self.data <= other

    def __gt__(self, other: Any) -> bool:
        if isinstance(other, UserString):
            return self.data > other.data
        return self.data > other

    def __ge__(self, other: Any) -> bool:
        if isinstance(other, UserString):
            return self.data >= other.data
        return self.data >= other

    # --- Sequence protocol ---

    def __len__(self) -> int:
        return len(self.data)

    def __getitem__(self, index) -> "UserString":
        return self.__class__(self.data[index])

    def __iter__(self):
        return iter(self.data)

    def __contains__(self, char: Any) -> bool:
        if isinstance(char, UserString):
            char = char.data
        return char in self.data

    # --- Concatenation / repetition ---

    def __add__(self, other: Any) -> "UserString":
        if isinstance(other, UserString):
            return self.__class__(self.data + other.data)
        if isinstance(other, str):
            return self.__class__(self.data + other)
        return NotImplemented

    def __radd__(self, other: Any) -> "UserString":
        if isinstance(other, str):
            return self.__class__(other + self.data)
        return NotImplemented

    def __mul__(self, n: int) -> "UserString":
        return self.__class__(self.data * n)

    def __rmul__(self, n: int) -> "UserString":
        return self.__class__(self.data * n)

    def __mod__(self, args: Any) -> "UserString":
        return self.__class__(self.data % args)

    # --- Format ---

    def __format__(self, format_spec: str) -> str:
        return self.data.__format__(format_spec)

    # --- String method delegations ---

    def capitalize(self) -> "UserString":
        return self.__class__(self.data.capitalize())

    def casefold(self) -> "UserString":
        return self.__class__(self.data.casefold())

    def center(self, width: int, *args) -> "UserString":
        return self.__class__(self.data.center(width, *args))

    def count(self, sub: Any, *args) -> int:
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.count(sub, *args)

    def encode(self, encoding: str = "utf-8", errors: str = "strict") -> bytes:
        return self.data.encode(encoding, errors)

    def endswith(self, suffix: Any, *args) -> bool:
        if isinstance(suffix, UserString):
            suffix = suffix.data
        return self.data.endswith(suffix, *args)

    def expandtabs(self, tabsize: int = 8) -> "UserString":
        return self.__class__(self.data.expandtabs(tabsize))

    def find(self, sub: Any, *args) -> int:
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.find(sub, *args)

    def format(self, /, *args, **kwds) -> "UserString":
        return self.__class__(self.data.format(*args, **kwds))

    def format_map(self, map: Any) -> "UserString":
        return self.__class__(self.data.format_map(map))

    def index(self, sub: Any, *args) -> int:
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.index(sub, *args)

    def isalnum(self) -> bool:
        return self.data.isalnum()

    def isalpha(self) -> bool:
        return self.data.isalpha()

    def isascii(self) -> bool:
        return self.data.isascii()

    def isdecimal(self) -> bool:
        return self.data.isdecimal()

    def isdigit(self) -> bool:
        return self.data.isdigit()

    def isidentifier(self) -> bool:
        return self.data.isidentifier()

    def islower(self) -> bool:
        return self.data.islower()

    def isnumeric(self) -> bool:
        return self.data.isnumeric()

    def isprintable(self) -> bool:
        return self.data.isprintable()

    def isspace(self) -> bool:
        return self.data.isspace()

    def istitle(self) -> bool:
        return self.data.istitle()

    def isupper(self) -> bool:
        return self.data.isupper()

    def join(self, iterable) -> "UserString":
        return self.__class__(self.data.join(iterable))

    def ljust(self, width: int, *args) -> "UserString":
        return self.__class__(self.data.ljust(width, *args))

    def lower(self) -> "UserString":
        return self.__class__(self.data.lower())

    def lstrip(self, chars: Any = None) -> "UserString":
        if isinstance(chars, UserString):
            chars = chars.data
        return self.__class__(self.data.lstrip(chars))

    def maketrans(self, *args):
        return self.data.maketrans(*args)

    def partition(self, sep: Any) -> tuple:
        if isinstance(sep, UserString):
            sep = sep.data
        return self.data.partition(sep)

    def removeprefix(self, prefix: Any) -> "UserString":
        if isinstance(prefix, UserString):
            prefix = prefix.data
        return self.__class__(self.data.removeprefix(prefix))

    def removesuffix(self, suffix: Any) -> "UserString":
        if isinstance(suffix, UserString):
            suffix = suffix.data
        return self.__class__(self.data.removesuffix(suffix))

    def replace(self, old: Any, new: Any, *args) -> "UserString":
        if isinstance(old, UserString):
            old = old.data
        if isinstance(new, UserString):
            new = new.data
        return self.__class__(self.data.replace(old, new, *args))

    def rfind(self, sub: Any, *args) -> int:
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.rfind(sub, *args)

    def rindex(self, sub: Any, *args) -> int:
        if isinstance(sub, UserString):
            sub = sub.data
        return self.data.rindex(sub, *args)

    def rjust(self, width: int, *args) -> "UserString":
        return self.__class__(self.data.rjust(width, *args))

    def rpartition(self, sep: Any) -> tuple:
        if isinstance(sep, UserString):
            sep = sep.data
        return self.data.rpartition(sep)

    def rsplit(self, sep: Any = None, maxsplit: int = -1) -> list:
        if isinstance(sep, UserString):
            sep = sep.data
        return self.data.rsplit(sep, maxsplit)

    def rstrip(self, chars: Any = None) -> "UserString":
        if isinstance(chars, UserString):
            chars = chars.data
        return self.__class__(self.data.rstrip(chars))

    def split(self, sep: Any = None, maxsplit: int = -1) -> list:
        if isinstance(sep, UserString):
            sep = sep.data
        return self.data.split(sep, maxsplit)

    def splitlines(self, keepends: bool = False) -> list:
        return self.data.splitlines(keepends)

    def startswith(self, prefix: Any, *args) -> bool:
        if isinstance(prefix, UserString):
            prefix = prefix.data
        return self.data.startswith(prefix, *args)

    def strip(self, chars: Any = None) -> "UserString":
        if isinstance(chars, UserString):
            chars = chars.data
        return self.__class__(self.data.strip(chars))

    def swapcase(self) -> "UserString":
        return self.__class__(self.data.swapcase())

    def title(self) -> "UserString":
        return self.__class__(self.data.title())

    def translate(self, *args) -> "UserString":
        return self.__class__(self.data.translate(*args))

    def upper(self) -> "UserString":
        return self.__class__(self.data.upper())

    def zfill(self, width: int) -> "UserString":
        return self.__class__(self.data.zfill(width))


globals().pop("_require_intrinsic", None)
