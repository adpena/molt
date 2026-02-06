"""Support to pretty-print lists, tuples, & dictionaries recursively.

Very simple, but useful, especially in debugging data structures.

Classes
-------

PrettyPrinter()
    Handle pretty-printing operations onto a stream using a configured
    set of formatting parameters.

Functions
---------

pformat()
    Format a Python object into a pretty-printed representation.

pprint()
    Pretty-print a Python object to a stream [default is sys.stdout].

saferepr()
    Generate a 'standard' repr()-like value, but protect against recursive
    data structures.

"""

from __future__ import annotations

from typing import Any, cast

from _intrinsics import require_intrinsic as _require_intrinsic

import collections as _collections
import sys as _sys
import types as _types
from io import StringIO as _StringIO

_require_intrinsic("molt_stdlib_probe", globals())


__all__ = [
    "pprint",
    "pformat",
    "isreadable",
    "isrecursive",
    "saferepr",
    "PrettyPrinter",
    "pp",
]


def pprint(
    object: object,
    stream=None,
    indent: int = 1,
    width: int = 80,
    depth: int | None = None,
    *,
    compact: bool = False,
    sort_dicts: bool = True,
    underscore_numbers: bool = False,
) -> None:
    """Pretty-print a Python object to a stream [default is sys.stdout]."""
    printer = PrettyPrinter(
        stream=stream,
        indent=indent,
        width=width,
        depth=depth,
        compact=compact,
        sort_dicts=sort_dicts,
        underscore_numbers=underscore_numbers,
    )
    printer.pprint(object)


def pformat(
    object: object,
    indent: int = 1,
    width: int = 80,
    depth: int | None = None,
    *,
    compact: bool = False,
    sort_dicts: bool = True,
    underscore_numbers: bool = False,
) -> str:
    """Format a Python object into a pretty-printed representation."""
    return PrettyPrinter(
        indent=indent,
        width=width,
        depth=depth,
        compact=compact,
        sort_dicts=sort_dicts,
        underscore_numbers=underscore_numbers,
    ).pformat(object)


def pp(object: object, *args, sort_dicts: bool = False, **kwargs) -> None:
    """Pretty-print a Python object."""
    pprint(object, *args, sort_dicts=sort_dicts, **kwargs)


def saferepr(object: object) -> str:
    """Version of repr() which can handle recursive data structures."""
    return PrettyPrinter()._safe_repr(object, {}, None, 0)[0]


def isreadable(object: object) -> bool:
    """Determine if saferepr(object) is readable by eval()."""
    return PrettyPrinter()._safe_repr(object, {}, None, 0)[1]


def isrecursive(object: object) -> bool:
    """Determine if object requires a recursive representation."""
    return PrettyPrinter()._safe_repr(object, {}, None, 0)[2]


def _sorted_dict_items(items):
    try:
        return sorted(items)
    except TypeError:

        def _dict_key(kv):
            return (str(type(kv[0])), id(kv[0]))

        return sorted(items, key=_dict_key)


def _sorted_set_items(values):
    try:
        return sorted(values)
    except TypeError:

        def _set_key(value):
            return (str(type(value)), id(value))

        return sorted(values, key=_set_key)


_DICT_REPR = getattr(dict, "__repr__", None)
_INT_REPR = getattr(int, "__repr__", None)
_LIST_REPR = getattr(list, "__repr__", None)
_TUPLE_REPR = getattr(tuple, "__repr__", None)
_SET_REPR = getattr(set, "__repr__", None)
_FROZENSET_REPR = getattr(frozenset, "__repr__", None)
_BYTES_REPR = getattr(bytes, "__repr__", None)
_BYTEARRAY_REPR = getattr(bytearray, "__repr__", None)

_MAPPINGPROXY_TYPE = getattr(_types, "MappingProxyType", None)
_MAPPINGPROXY_REPR = (
    getattr(_MAPPINGPROXY_TYPE, "__repr__", None) if _MAPPINGPROXY_TYPE else None
)
_SIMPLENAMESPACE_TYPE = getattr(_types, "SimpleNamespace", None)
_SIMPLENAMESPACE_REPR = (
    getattr(_SIMPLENAMESPACE_TYPE, "__repr__", None) if _SIMPLENAMESPACE_TYPE else None
)

_ORDEREDDICT_TYPE = getattr(_collections, "OrderedDict", None)
_ORDEREDDICT_REPR = (
    getattr(_ORDEREDDICT_TYPE, "__repr__", None) if _ORDEREDDICT_TYPE else None
)
_DEFAULTDICT_TYPE = getattr(_collections, "defaultdict", None)
_DEFAULTDICT_REPR = (
    getattr(_DEFAULTDICT_TYPE, "__repr__", None) if _DEFAULTDICT_TYPE else None
)
_COUNTER_TYPE = getattr(_collections, "Counter", None)
_COUNTER_REPR = getattr(_COUNTER_TYPE, "__repr__", None) if _COUNTER_TYPE else None
_CHAINMAP_TYPE = getattr(_collections, "ChainMap", None)
_CHAINMAP_REPR = getattr(_CHAINMAP_TYPE, "__repr__", None) if _CHAINMAP_TYPE else None
_DEQUE_TYPE = getattr(_collections, "deque", None)
_DEQUE_REPR = getattr(_DEQUE_TYPE, "__repr__", None) if _DEQUE_TYPE else None
_USERDICT_TYPE = getattr(_collections, "UserDict", None)
_USERDICT_REPR = getattr(_USERDICT_TYPE, "__repr__", None) if _USERDICT_TYPE else None
_USERLIST_TYPE = getattr(_collections, "UserList", None)
_USERLIST_REPR = getattr(_USERLIST_TYPE, "__repr__", None) if _USERLIST_TYPE else None
_USERSTRING_TYPE = getattr(_collections, "UserString", None)
_USERSTRING_REPR = (
    getattr(_USERSTRING_TYPE, "__repr__", None) if _USERSTRING_TYPE else None
)

_IS_DATACLASS = None


def _get_is_dataclass():
    global _IS_DATACLASS
    if _IS_DATACLASS is False:
        return None
    if _IS_DATACLASS is None:
        try:
            from dataclasses import is_dataclass as _is_dataclass
        except Exception:
            _IS_DATACLASS = False
            return None
        _IS_DATACLASS = _is_dataclass
    return _IS_DATACLASS


class PrettyPrinter:
    def __init__(
        self,
        indent: int = 1,
        width: int = 80,
        depth: int | None = None,
        stream=None,
        *,
        compact: bool = False,
        sort_dicts: bool = True,
        underscore_numbers: bool = False,
    ) -> None:
        """Handle pretty printing operations onto a stream using a set of
        configured parameters.

        indent
            Number of spaces to indent for each level of nesting.

        width
            Attempted maximum number of columns in the output.

        depth
            The maximum depth to print out nested structures.

        stream
            The desired output stream.  If omitted (or false), the standard
            output stream available at construction will be used.

        compact
            If true, several items will be combined in one line.

        sort_dicts
            If true, dict keys are sorted.

        underscore_numbers
            If true, digit groups are separated with underscores.

        """
        indent = int(indent)
        width = int(width)
        if indent < 0:
            raise ValueError("indent must be >= 0")
        if depth is not None and depth <= 0:
            raise ValueError("depth must be > 0")
        if not width:
            raise ValueError("width must be != 0")
        self._depth = depth
        self._indent_per_level = indent
        self._width = width
        self._stream = _sys.stdout if stream is None else stream
        self._compact = bool(compact)
        self._sort_dicts = sort_dicts
        self._underscore_numbers = underscore_numbers
        self._readable = True
        self._recursive = False

    def pprint(self, object: object) -> None:
        if self._stream is not None:
            self._format(object, self._stream, 0, 0, {}, 0)
            self._stream.write("\n")

    def pformat(self, object: object) -> str:
        sio = _StringIO()
        self._format(object, sio, 0, 0, {}, 0)
        return sio.getvalue()

    def isrecursive(self, object: object) -> bool:
        return self.format(object, {}, 0, 0)[2]

    def isreadable(self, object: object) -> bool:
        s, readable, recursive = self.format(object, {}, 0, 0)
        return readable and not recursive

    def _format(
        self,
        object: object,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        objid = id(object)
        if objid in context:
            stream.write(_recursion(object))
            self._recursive = True
            self._readable = False
            return
        rep = self._repr(object, context, level)
        max_width = self._width - indent - allowance
        if len(rep) > max_width:
            repr_fn = getattr(type(object), "__repr__", None)
            dispatch = PrettyPrinter._dispatch
            p = dispatch.get(repr_fn)
            if p is None:
                p = dispatch.get(type(object))
            is_dataclass = _get_is_dataclass()

            if p is not None:
                context[objid] = 1
                p(self, object, stream, indent, allowance, context, level + 1)
                del context[objid]
                return
            if (
                is_dataclass is not None
                and is_dataclass(object)
                and not isinstance(object, type)
            ):
                params = getattr(object, "__dataclass_params__", None)
                wrapped = getattr(
                    getattr(object, "__repr__", None), "__wrapped__", None
                )
                qualname = getattr(wrapped, "__qualname__", "")
                if (
                    params is not None
                    and getattr(params, "repr", False)
                    and wrapped is not None
                    and "__create_fn__" in qualname
                ):
                    context[objid] = 1
                    self._pprint_dataclass(
                        object, stream, indent, allowance, context, level + 1
                    )
                    del context[objid]
                    return
        stream.write(rep)

    def _pprint_dataclass(
        self,
        object: object,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        # Lazy import to improve module import time
        from dataclasses import fields as dataclass_fields

        cls_name = object.__class__.__name__
        indent += len(cls_name) + 1
        items = [
            (f.name, getattr(object, f.name))
            for f in dataclass_fields(cast(Any, object))
            if f.repr
        ]
        stream.write(cls_name + "(")
        self._format_namespace_items(items, stream, indent, allowance, context, level)
        stream.write(")")

    _dispatch: dict[object, object] = {}

    def _pprint_dict(
        self,
        object: dict[object, object],
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        write("{")
        if self._indent_per_level > 1:
            write((self._indent_per_level - 1) * " ")
        length = len(object)
        if length:
            if self._sort_dicts:
                items = _sorted_dict_items(object.items())
            else:
                items = object.items()
            self._format_dict_items(
                items, stream, indent, allowance + 1, context, level
            )
        write("}")

    if _DICT_REPR is not None:
        _dispatch[_DICT_REPR] = _pprint_dict
    _dispatch[dict] = _pprint_dict

    def _pprint_ordered_dict(
        self,
        object: _collections.OrderedDict,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object):
            stream.write(repr(object))
            return
        cls = object.__class__
        stream.write(cls.__name__ + "(")
        self._format(
            list(object.items()),
            stream,
            indent + len(cls.__name__) + 1,
            allowance + 1,
            context,
            level,
        )
        stream.write(")")

    if _ORDEREDDICT_REPR is not None:
        _dispatch[_ORDEREDDICT_REPR] = _pprint_ordered_dict
    if _ORDEREDDICT_TYPE is not None:
        _dispatch[_ORDEREDDICT_TYPE] = _pprint_ordered_dict

    def _pprint_list(
        self,
        object: list[object],
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        stream.write("[")
        self._format_items(object, stream, indent, allowance + 1, context, level)
        stream.write("]")

    if _LIST_REPR is not None:
        _dispatch[_LIST_REPR] = _pprint_list
    _dispatch[list] = _pprint_list

    def _pprint_tuple(
        self,
        object: tuple[object, ...],
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        stream.write("(")
        endchar = ",)" if len(object) == 1 else ")"
        self._format_items(
            object, stream, indent, allowance + len(endchar), context, level
        )
        stream.write(endchar)

    if _TUPLE_REPR is not None:
        _dispatch[_TUPLE_REPR] = _pprint_tuple
    _dispatch[tuple] = _pprint_tuple

    def _pprint_set(
        self,
        object: set[object] | frozenset[object],
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object):
            stream.write(repr(object))
            return
        typ = object.__class__
        if typ is set:
            stream.write("{")
            endchar = "}"
        else:
            stream.write(typ.__name__ + "({")
            endchar = "})"
            indent += len(typ.__name__) + 1
        object = _sorted_set_items(object)
        self._format_items(
            object, stream, indent, allowance + len(endchar), context, level
        )
        stream.write(endchar)

    if _SET_REPR is not None:
        _dispatch[_SET_REPR] = _pprint_set
    if _FROZENSET_REPR is not None:
        _dispatch[_FROZENSET_REPR] = _pprint_set
    _dispatch[set] = _pprint_set
    _dispatch[frozenset] = _pprint_set

    def _pprint_str(
        self,
        object: str,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        if not len(object):
            write(repr(object))
            return
        chunks: list[str] = []
        lines = object.splitlines(True)
        if level == 1:
            indent += 1
            allowance += 1
        max_width1 = max_width = self._width - indent
        rep = ""
        for i, line in enumerate(lines):
            rep = repr(line)
            if i == len(lines) - 1:
                max_width1 -= allowance
            if len(rep) <= max_width1:
                chunks.append(rep)
            else:
                # Lazy import to improve module import time
                import re

                parts = re.findall(r"\S*\s*", line)
                if parts and not parts[-1]:
                    parts.pop()
                max_width2 = max_width
                current = ""
                for j, part in enumerate(parts):
                    candidate = current + part
                    if j == len(parts) - 1 and i == len(lines) - 1:
                        max_width2 -= allowance
                    if len(repr(candidate)) > max_width2:
                        if current:
                            chunks.append(repr(current))
                        current = part
                    else:
                        current = candidate
                if current:
                    chunks.append(repr(current))
        if len(chunks) == 1:
            write(rep)
            return
        if level == 1:
            write("(")
        for i, rep in enumerate(chunks):
            if i > 0:
                write("\n" + " " * indent)
            write(rep)
        if level == 1:
            write(")")

    _dispatch[str] = _pprint_str

    def _pprint_bytes(
        self,
        object: bytes,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        if len(object) <= 4:
            write(repr(object))
            return
        parens = level == 1
        if parens:
            indent += 1
            allowance += 1
            write("(")
        delim = ""
        for rep in _wrap_bytes_repr(object, self._width - indent, allowance):
            write(delim)
            write(rep)
            if not delim:
                delim = "\n" + " " * indent
        if parens:
            write(")")

    if _BYTES_REPR is not None:
        _dispatch[_BYTES_REPR] = _pprint_bytes
    _dispatch[bytes] = _pprint_bytes

    def _pprint_bytearray(
        self,
        object: bytearray,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        write("bytearray(")
        self._pprint_bytes(
            bytes(object), stream, indent + 10, allowance + 1, context, level + 1
        )
        write(")")

    if _BYTEARRAY_REPR is not None:
        _dispatch[_BYTEARRAY_REPR] = _pprint_bytearray
    _dispatch[bytearray] = _pprint_bytearray

    def _pprint_mappingproxy(
        self,
        object: _types.MappingProxyType,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        stream.write("mappingproxy(")
        self._format(object.copy(), stream, indent + 13, allowance + 1, context, level)
        stream.write(")")

    if _MAPPINGPROXY_REPR is not None:
        _dispatch[_MAPPINGPROXY_REPR] = _pprint_mappingproxy
    if _MAPPINGPROXY_TYPE is not None:
        _dispatch[_MAPPINGPROXY_TYPE] = _pprint_mappingproxy

    def _pprint_simplenamespace(
        self,
        object: _types.SimpleNamespace,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if _SIMPLENAMESPACE_TYPE is not None and type(object) is _SIMPLENAMESPACE_TYPE:
            cls_name = "namespace"
        else:
            cls_name = object.__class__.__name__
        indent += len(cls_name) + 1
        items = object.__dict__.items()
        stream.write(cls_name + "(")
        self._format_namespace_items(items, stream, indent, allowance, context, level)
        stream.write(")")

    if _SIMPLENAMESPACE_REPR is not None:
        _dispatch[_SIMPLENAMESPACE_REPR] = _pprint_simplenamespace
    if _SIMPLENAMESPACE_TYPE is not None:
        _dispatch[_SIMPLENAMESPACE_TYPE] = _pprint_simplenamespace

    def _format_dict_items(
        self,
        items,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        indent += self._indent_per_level
        delimnl = ",\n" + " " * indent
        last_index = len(items) - 1
        for i, (key, ent) in enumerate(items):
            last = i == last_index
            rep = self._repr(key, context, level)
            write(rep)
            write(": ")
            self._format(
                ent,
                stream,
                indent + len(rep) + 2,
                allowance if last else 1,
                context,
                level,
            )
            if not last:
                write(delimnl)

    def _format_namespace_items(
        self,
        items,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        delimnl = ",\n" + " " * indent
        last_index = len(items) - 1
        for i, (key, ent) in enumerate(items):
            last = i == last_index
            write(key)
            write("=")
            if id(ent) in context:
                write("...")
            else:
                self._format(
                    ent,
                    stream,
                    indent + len(key) + 1,
                    allowance if last else 1,
                    context,
                    level,
                )
            if not last:
                write(delimnl)

    def _format_items(
        self,
        items,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        write = stream.write
        indent += self._indent_per_level
        if self._indent_per_level > 1:
            write((self._indent_per_level - 1) * " ")
        delimnl = ",\n" + " " * indent
        delim = ""
        width = max_width = self._width - indent + 1
        it = iter(items)
        try:
            next_ent = next(it)
        except StopIteration:
            return
        last = False
        while not last:
            ent = next_ent
            try:
                next_ent = next(it)
            except StopIteration:
                last = True
                max_width -= allowance
                width -= allowance
            if self._compact:
                rep = self._repr(ent, context, level)
                w = len(rep) + 2
                if width < w:
                    width = max_width
                    if delim:
                        delim = delimnl
                if width >= w:
                    width -= w
                    write(delim)
                    delim = ", "
                    write(rep)
                    continue
            write(delim)
            delim = delimnl
            self._format(ent, stream, indent, allowance if last else 1, context, level)

    def _repr(self, object: object, context: dict[int, int], level: int) -> str:
        rep, readable, recursive = self.format(object, context, self._depth, level)
        if not readable:
            self._readable = False
        if recursive:
            self._recursive = True
        return rep

    def format(
        self,
        object: object,
        context: dict[int, int],
        maxlevels: int | None,
        level: int,
    ) -> tuple[str, bool, bool]:
        """Format object for a specific context, returning a string
        and flags indicating whether the representation is 'readable'
        and whether the object represents a recursive construct.
        """
        return self._safe_repr(object, context, maxlevels, level)

    def _pprint_default_dict(
        self,
        object: _collections.defaultdict,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object):
            stream.write(repr(object))
            return
        rdf = self._repr(object.default_factory, context, level)
        cls = object.__class__
        indent += len(cls.__name__) + 1
        stream.write(f"{cls.__name__}({rdf},\n" + " " * indent)
        self._pprint_dict(object, stream, indent, allowance + 1, context, level)
        stream.write(")")

    if _DEFAULTDICT_REPR is not None:
        _dispatch[_DEFAULTDICT_REPR] = _pprint_default_dict
    if _DEFAULTDICT_TYPE is not None:
        _dispatch[_DEFAULTDICT_TYPE] = _pprint_default_dict

    def _pprint_counter(
        self,
        object: _collections.Counter,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object):
            stream.write(repr(object))
            return
        cls = object.__class__
        stream.write(cls.__name__ + "({")
        if self._indent_per_level > 1:
            stream.write((self._indent_per_level - 1) * " ")
        items = object.most_common()
        self._format_dict_items(
            items, stream, indent + len(cls.__name__) + 1, allowance + 2, context, level
        )
        stream.write("})")

    if _COUNTER_REPR is not None:
        _dispatch[_COUNTER_REPR] = _pprint_counter
    if _COUNTER_TYPE is not None:
        _dispatch[_COUNTER_TYPE] = _pprint_counter

    def _pprint_chain_map(
        self,
        object: _collections.ChainMap,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object.maps):
            stream.write(repr(object))
            return
        cls = object.__class__
        stream.write(cls.__name__ + "(")
        indent += len(cls.__name__) + 1
        for i, mapping in enumerate(object.maps):
            if i == len(object.maps) - 1:
                self._format(mapping, stream, indent, allowance + 1, context, level)
                stream.write(")")
            else:
                self._format(mapping, stream, indent, 1, context, level)
                stream.write(",\n" + " " * indent)

    if _CHAINMAP_REPR is not None:
        _dispatch[_CHAINMAP_REPR] = _pprint_chain_map
    if _CHAINMAP_TYPE is not None:
        _dispatch[_CHAINMAP_TYPE] = _pprint_chain_map

    def _pprint_deque(
        self,
        object: _collections.deque,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        if not len(object):
            stream.write(repr(object))
            return
        cls = object.__class__
        stream.write(cls.__name__ + "(")
        indent += len(cls.__name__) + 1
        stream.write("[")
        if object.maxlen is None:
            self._format_items(object, stream, indent, allowance + 2, context, level)
            stream.write("])")
        else:
            self._format_items(object, stream, indent, 2, context, level)
            rml = self._repr(object.maxlen, context, level)
            stream.write("],\n" + " " * indent + f"maxlen={rml})")

    if _DEQUE_REPR is not None:
        _dispatch[_DEQUE_REPR] = _pprint_deque
    if _DEQUE_TYPE is not None:
        _dispatch[_DEQUE_TYPE] = _pprint_deque

    def _pprint_user_dict(
        self,
        object: _collections.UserDict,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        self._format(object.data, stream, indent, allowance, context, level - 1)

    if _USERDICT_REPR is not None:
        _dispatch[_USERDICT_REPR] = _pprint_user_dict
    if _USERDICT_TYPE is not None:
        _dispatch[_USERDICT_TYPE] = _pprint_user_dict

    def _pprint_user_list(
        self,
        object: _collections.UserList,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        self._format(object.data, stream, indent, allowance, context, level - 1)

    if _USERLIST_REPR is not None:
        _dispatch[_USERLIST_REPR] = _pprint_user_list
    if _USERLIST_TYPE is not None:
        _dispatch[_USERLIST_TYPE] = _pprint_user_list

    def _pprint_user_string(
        self,
        object: _collections.UserString,
        stream,
        indent: int,
        allowance: int,
        context: dict[int, int],
        level: int,
    ) -> None:
        self._format(object.data, stream, indent, allowance, context, level - 1)

    if _USERSTRING_REPR is not None:
        _dispatch[_USERSTRING_REPR] = _pprint_user_string
    if _USERSTRING_TYPE is not None:
        _dispatch[_USERSTRING_TYPE] = _pprint_user_string

    def _safe_repr(
        self,
        object: object,
        context: dict[int, int],
        maxlevels: int | None,
        level: int,
    ) -> tuple[str, bool, bool]:
        typ = type(object)
        if typ in _builtin_scalars:
            return repr(object), True, False

        r = getattr(typ, "__repr__", None)

        if typ is int and ((r is _INT_REPR) or (_INT_REPR is None)):
            if self._underscore_numbers:
                return f"{object:_d}", True, False
            return repr(object), True, False

        if typ is dict and ((r is _DICT_REPR) or (_DICT_REPR is None)):
            obj_dict = cast(dict[Any, Any], object)
            if not obj_dict:
                return "{}", True, False
            objid = id(object)
            if maxlevels and level >= maxlevels:
                return "{...}", False, objid in context
            if objid in context:
                return _recursion(object), False, True
            context[objid] = 1
            readable = True
            recursive = False
            components = []
            append = components.append
            level += 1
            if self._sort_dicts:
                items = _sorted_dict_items(obj_dict.items())
            else:
                items = obj_dict.items()
            for key, value in items:
                krepr, kreadable, krecur = self.format(key, context, maxlevels, level)
                vrepr, vreadable, vrecur = self.format(value, context, maxlevels, level)
                append(f"{krepr}: {vrepr}")
                readable = readable and kreadable and vreadable
                if krecur or vrecur:
                    recursive = True
            del context[objid]
            return "{" + ", ".join(components) + "}", readable, recursive

        if typ is list or typ is tuple:
            seq = cast(list[Any] | tuple[Any, ...], object)
            if typ is list:
                if not seq:
                    return "[]", True, False
                open_char = "["
                close_char = "]"
            elif len(seq) == 1:
                open_char = "("
                close_char = ",)"
            else:
                if not seq:
                    return "()", True, False
                open_char = "("
                close_char = ")"
            objid = id(object)
            if maxlevels and level >= maxlevels:
                return f"{open_char}...{close_char}", False, objid in context
            if objid in context:
                return _recursion(object), False, True
            context[objid] = 1
            readable = True
            recursive = False
            components = []
            append = components.append
            level += 1
            for entry in seq:
                erepr, ereadable, erecur = self.format(entry, context, maxlevels, level)
                append(erepr)
                if not ereadable:
                    readable = False
                if erecur:
                    recursive = True
            del context[objid]
            return (
                f"{open_char}{', '.join(components)}{close_char}",
                readable,
                recursive,
            )

        rep = repr(object)
        return rep, bool(rep and not rep.startswith("<")), False


def _ensure_dispatch() -> None:
    dispatch = PrettyPrinter._dispatch
    if _DICT_REPR is not None:
        dispatch[_DICT_REPR] = PrettyPrinter._pprint_dict
    dispatch[dict] = PrettyPrinter._pprint_dict
    if _ORDEREDDICT_REPR is not None:
        dispatch[_ORDEREDDICT_REPR] = PrettyPrinter._pprint_ordered_dict
    if _ORDEREDDICT_TYPE is not None:
        dispatch[_ORDEREDDICT_TYPE] = PrettyPrinter._pprint_ordered_dict
    if _LIST_REPR is not None:
        dispatch[_LIST_REPR] = PrettyPrinter._pprint_list
    dispatch[list] = PrettyPrinter._pprint_list
    if _TUPLE_REPR is not None:
        dispatch[_TUPLE_REPR] = PrettyPrinter._pprint_tuple
    dispatch[tuple] = PrettyPrinter._pprint_tuple
    if _SET_REPR is not None:
        dispatch[_SET_REPR] = PrettyPrinter._pprint_set
    if _FROZENSET_REPR is not None:
        dispatch[_FROZENSET_REPR] = PrettyPrinter._pprint_set
    dispatch[set] = PrettyPrinter._pprint_set
    dispatch[frozenset] = PrettyPrinter._pprint_set
    dispatch[str] = PrettyPrinter._pprint_str
    if _BYTES_REPR is not None:
        dispatch[_BYTES_REPR] = PrettyPrinter._pprint_bytes
    dispatch[bytes] = PrettyPrinter._pprint_bytes
    if _BYTEARRAY_REPR is not None:
        dispatch[_BYTEARRAY_REPR] = PrettyPrinter._pprint_bytearray
    dispatch[bytearray] = PrettyPrinter._pprint_bytearray
    if _MAPPINGPROXY_REPR is not None:
        dispatch[_MAPPINGPROXY_REPR] = PrettyPrinter._pprint_mappingproxy
    if _MAPPINGPROXY_TYPE is not None:
        dispatch[_MAPPINGPROXY_TYPE] = PrettyPrinter._pprint_mappingproxy
    if _SIMPLENAMESPACE_REPR is not None:
        dispatch[_SIMPLENAMESPACE_REPR] = PrettyPrinter._pprint_simplenamespace
    if _SIMPLENAMESPACE_TYPE is not None:
        dispatch[_SIMPLENAMESPACE_TYPE] = PrettyPrinter._pprint_simplenamespace
    if _DEFAULTDICT_REPR is not None:
        dispatch[_DEFAULTDICT_REPR] = PrettyPrinter._pprint_default_dict
    if _DEFAULTDICT_TYPE is not None:
        dispatch[_DEFAULTDICT_TYPE] = PrettyPrinter._pprint_default_dict
    if _COUNTER_REPR is not None:
        dispatch[_COUNTER_REPR] = PrettyPrinter._pprint_counter
    if _COUNTER_TYPE is not None:
        dispatch[_COUNTER_TYPE] = PrettyPrinter._pprint_counter
    if _CHAINMAP_REPR is not None:
        dispatch[_CHAINMAP_REPR] = PrettyPrinter._pprint_chain_map
    if _CHAINMAP_TYPE is not None:
        dispatch[_CHAINMAP_TYPE] = PrettyPrinter._pprint_chain_map
    if _DEQUE_REPR is not None:
        dispatch[_DEQUE_REPR] = PrettyPrinter._pprint_deque
    if _DEQUE_TYPE is not None:
        dispatch[_DEQUE_TYPE] = PrettyPrinter._pprint_deque
    if _USERDICT_REPR is not None:
        dispatch[_USERDICT_REPR] = PrettyPrinter._pprint_user_dict
    if _USERDICT_TYPE is not None:
        dispatch[_USERDICT_TYPE] = PrettyPrinter._pprint_user_dict
    if _USERLIST_REPR is not None:
        dispatch[_USERLIST_REPR] = PrettyPrinter._pprint_user_list
    if _USERLIST_TYPE is not None:
        dispatch[_USERLIST_TYPE] = PrettyPrinter._pprint_user_list
    if _USERSTRING_REPR is not None:
        dispatch[_USERSTRING_REPR] = PrettyPrinter._pprint_user_string
    if _USERSTRING_TYPE is not None:
        dispatch[_USERSTRING_TYPE] = PrettyPrinter._pprint_user_string


_ensure_dispatch()


_builtin_scalars = frozenset({str, bytes, bytearray, float, complex, bool, type(None)})


def _recursion(object: object) -> str:
    return f"<Recursion on {type(object).__name__} with id={id(object)}>"


def _wrap_bytes_repr(object: bytes, width: int, allowance: int):
    current = b""
    last = len(object) // 4 * 4
    for i in range(0, len(object), 4):
        part = object[i : i + 4]
        candidate = current + part
        if i == last:
            width -= allowance
        if len(repr(candidate)) > width:
            if current:
                yield repr(current)
            current = part
        else:
            current = candidate
    if current:
        yield repr(current)
