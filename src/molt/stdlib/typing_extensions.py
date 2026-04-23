"""Deterministic `typing_extensions` surface for Molt.

This module is a runtime compatibility layer for type-hint-heavy packages.
It does not import the external PyPI package and does not use host fallback
imports. Runtime-private typing objects come from Molt's intrinsic typing
payload; missing intrinsic support raises at import time.
"""

from __future__ import annotations

import builtins as _builtins
import collections as _collections
import collections.abc as _abc
import enum as _enum
import re as _re
import sys as _sys
import types as _types
import typing as _typing

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TYPING_PRIVATE_PAYLOAD = _require_intrinsic("molt_typing_private_payload")
_TYPING_PAYLOAD = _MOLT_TYPING_PRIVATE_PAYLOAD(_typing)


def _payload_get(name: str) -> object:
    if not isinstance(_TYPING_PAYLOAD, dict):
        raise RuntimeError("typing_extensions expected dict typing payload")
    if name not in _TYPING_PAYLOAD:
        raise RuntimeError(f"typing_extensions typing payload missing {name}")
    return _TYPING_PAYLOAD[name]


_MISSING = object()


def _typing_attr(name: str, default: object = _MISSING) -> object:
    value = getattr(_typing, name, _MISSING)
    if value is not _MISSING:
        return value
    if default is not _MISSING:
        return default
    raise RuntimeError(f"typing_extensions requires typing.{name}")


class _SimpleAlias:
    __slots__ = ("__origin__", "__args__", "_name")

    def __init__(self, origin: object, args: object, name: str) -> None:
        self.__origin__ = origin
        if isinstance(args, tuple):
            self.__args__ = args
        else:
            self.__args__ = (args,)
        self._name = name

    def __repr__(self) -> str:
        return f"typing_extensions.{self._name}[{', '.join(repr(a) for a in self.__args__)}]"

    def __mro_entries__(self, _bases: tuple[object, ...]) -> tuple[object, ...]:
        origin = self.__origin__
        if isinstance(origin, type):
            return (origin,)
        return ()


class _SimpleSpecialForm:
    __slots__ = ("_name",)

    def __init__(self, name: str) -> None:
        self._name = name

    def __repr__(self) -> str:
        return f"typing_extensions.{self._name}"

    def __getitem__(self, params: object) -> _SimpleAlias:
        return _SimpleAlias(self, params, self._name)

    def __call__(self, *_args: object, **_kwargs: object) -> object:
        raise TypeError(f"Cannot instantiate {self!r}")


class _SimpleSpecialGenericAlias:
    __slots__ = ("__origin__", "_name")

    def __init__(self, origin: object, name: str) -> None:
        self.__origin__ = origin
        self._name = name

    def __repr__(self) -> str:
        return f"typing_extensions.{self._name}"

    def __getitem__(self, params: object) -> _SimpleAlias:
        return _SimpleAlias(self.__origin__, params, self._name)


def _identity(value):
    return value


def _deprecated(reason=None, /, *, category=None, stacklevel: int = 1):
    def decorator(obj):
        setattr(obj, "__deprecated__", reason)
        return obj

    return decorator


Annotated = _typing_attr("Annotated")
Any = _typing_attr("Any")
Awaitable = _typing_attr("Awaitable")
BinaryIO = _typing_attr("BinaryIO")
Callable = _typing_attr("Callable")
ClassVar = _typing_attr("ClassVar")
Concatenate = _typing_attr("Concatenate")
Dict = _typing_attr("Dict")
Final = _typing_attr("Final")
ForwardRef = _typing_attr("ForwardRef")
FrozenSet = _typing_attr("FrozenSet")
Generic = _typing_attr("Generic")
IO = _typing_attr("IO")
Iterable = _typing_attr("Iterable")
Iterator = _typing_attr("Iterator")
List = _typing_attr("List")
Literal = _typing_attr("Literal")
LiteralString = _typing_attr("LiteralString")
MutableMapping = _typing_attr("MutableMapping")
NamedTuple = _typing_attr("NamedTuple")
Never = _typing_attr("Never")
NewType = _typing_attr("NewType")
NoReturn = _typing_attr("NoReturn")
NotRequired = _typing_attr("NotRequired")
Optional = _typing_attr("Optional")
ParamSpec = _typing_attr("ParamSpec")
Protocol = _typing_attr("Protocol")
Required = _typing_attr("Required")
Self = _typing_attr("Self")
Set = _typing_attr("Set")
SupportsIndex = _typing_attr("SupportsIndex")
SupportsInt = _typing_attr("SupportsInt")
TYPE_CHECKING = bool(_typing_attr("TYPE_CHECKING", False))
Text = _typing_attr("Text")
TextIO = _typing_attr("TextIO")
Tuple = _typing_attr("Tuple")
TypeAlias = _typing_attr("TypeAlias")
TypeGuard = _typing_attr("TypeGuard")
TypeVar = _typing_attr("TypeVar")
TypeVarTuple = _typing_attr("TypeVarTuple")
TypedDict = _typing_attr("TypedDict")
Union = _typing_attr("Union")

assert_never = _typing_attr("assert_never")
assert_type = _typing_attr("assert_type")
cast = _typing_attr("cast")
clear_overloads = _typing_attr("clear_overloads")
dataclass_transform = _typing_attr("dataclass_transform")
get_overloads = _typing_attr("get_overloads")
get_type_hints = _typing_attr("get_type_hints")
is_typeddict = _typing_attr("is_typeddict")
overload = _typing_attr("overload")
runtime_checkable = _typing_attr("runtime_checkable")

deprecated = _typing_attr("deprecated", _deprecated)
final = _typing_attr("final", _identity)
no_type_check = _typing_attr("no_type_check", _identity)
no_type_check_decorator = _typing_attr("no_type_check_decorator", _identity)
override = _typing_attr("override", _identity)
reveal_type = _typing_attr("reveal_type", _identity)

ParamSpecArgs = _payload_get("ParamSpecArgs")
ParamSpecKwargs = _payload_get("ParamSpecKwargs")
TypeAliasType = _payload_get("TypeAliasType")

AnyStr = _typing_attr("AnyStr", TypeVar("AnyStr", bytes, str))
Type = _typing_attr("Type", _SimpleSpecialGenericAlias(type, "Type"))
Unpack = _typing_attr("Unpack", _SimpleSpecialForm("Unpack"))
ReadOnly = _typing_attr("ReadOnly", _SimpleSpecialForm("ReadOnly"))
TypeIs = _typing_attr("TypeIs", _SimpleSpecialForm("TypeIs"))
TypeForm = _typing_attr("TypeForm", _SimpleSpecialForm("TypeForm"))
NoExtraItems = _typing_attr("NoExtraItems", _SimpleSpecialForm("NoExtraItems"))


class _NoDefaultType:
    __slots__ = ()

    def __repr__(self) -> str:
        return "typing_extensions.NoDefault"

    def __bool__(self) -> bool:
        return False


NoDefault = _typing_attr("NoDefault", _NoDefaultType())


def _abc_or_typing(abc_name: str, typing_name: str | None = None) -> object:
    value = getattr(_abc, abc_name, _MISSING)
    if value is not _MISSING:
        return value
    return _typing_attr(typing_name or abc_name, object)


AbstractSet = _abc_or_typing("Set", "AbstractSet")
AsyncContextManager = _typing_attr("AsyncContextManager", object)
AsyncGenerator = _abc_or_typing("AsyncGenerator")
AsyncIterable = _abc_or_typing("AsyncIterable")
AsyncIterator = _abc_or_typing("AsyncIterator")
Collection = _abc_or_typing("Collection")
Container = _abc_or_typing("Container")
ContextManager = _typing_attr("ContextManager", object)
Coroutine = _abc_or_typing("Coroutine")
Generator = _abc_or_typing("Generator")
Hashable = _abc_or_typing("Hashable")
ItemsView = _abc_or_typing("ItemsView")
KeysView = _abc_or_typing("KeysView")
Mapping = _abc_or_typing("Mapping")
MappingView = _abc_or_typing("MappingView")
MutableSequence = _abc_or_typing("MutableSequence")
MutableSet = _abc_or_typing("MutableSet")
Reversible = _abc_or_typing("Reversible")
Sequence = _abc_or_typing("Sequence")
Sized = _abc_or_typing("Sized")
ValuesView = _abc_or_typing("ValuesView")

ChainMap = _collections.ChainMap
Counter = _collections.Counter
DefaultDict = _collections.defaultdict
Deque = _collections.deque
OrderedDict = _collections.OrderedDict

Match = _re.Match
Pattern = _re.Pattern


@runtime_checkable
class SupportsAbs(Protocol):
    def __abs__(self): ...


@runtime_checkable
class SupportsBytes(Protocol):
    def __bytes__(self) -> bytes: ...


@runtime_checkable
class SupportsComplex(Protocol):
    def __complex__(self) -> complex: ...


@runtime_checkable
class SupportsFloat(Protocol):
    def __float__(self) -> float: ...


@runtime_checkable
class SupportsRound(Protocol):
    def __round__(self, ndigits: int = 0): ...


@runtime_checkable
class Buffer(Protocol):
    def __buffer__(self, flags: int) -> memoryview: ...


class CapsuleType:
    """Placeholder runtime type for PyCapsule objects."""


class Format(_enum.IntEnum):
    VALUE = 1
    VALUE_WITH_FAKE_GLOBALS = 2
    FORWARDREF = 3
    STRING = 4


class Doc:
    __slots__ = ("documentation",)

    def __init__(self, documentation: str, /) -> None:
        if not isinstance(documentation, str):
            raise TypeError("Doc() requires a str argument")
        self.documentation = documentation

    def __repr__(self) -> str:
        return f"Doc({self.documentation!r})"

    def __hash__(self) -> int:
        return hash(self.documentation)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Doc):
            return NotImplemented
        return self.documentation == other.documentation


class Sentinel:
    __slots__ = ("_name", "_repr")

    def __init__(self, name: str, repr: str | None = None) -> None:  # noqa: A002
        if not isinstance(name, str):
            raise TypeError("Sentinel() requires a str name")
        self._name = name
        self._repr = repr if repr is not None else f"<{name}>"

    def __repr__(self) -> str:
        return self._repr

    def __getstate__(self):
        raise TypeError(f"Cannot pickle {type(self).__name__!r} object")


@runtime_checkable
class Reader(Protocol):
    def read(self, n: int = -1, /) -> str: ...


@runtime_checkable
class Writer(Protocol):
    def write(self, s: str, /) -> int: ...


runtime = runtime_checkable


def IntVar(name: str) -> object:
    return TypeVar(name)


def disjoint_base(cls: type) -> type:
    cls.__disjoint_base__ = True
    return cls


def type_repr(value: object) -> str:
    if isinstance(value, (type, _types.FunctionType, _types.BuiltinFunctionType)):
        module = getattr(value, "__module__", None)
        qualname = getattr(value, "__qualname__", None) or repr(value)
        if module == "builtins":
            return qualname
        if module:
            return f"{module}.{qualname}"
        return qualname
    if value is ...:
        return "..."
    return repr(value)


def get_original_bases(cls: type, /) -> tuple:
    if not isinstance(cls, type):
        raise TypeError(f"Expected an instance of type, not {type(cls).__name__!r}")
    return cls.__dict__.get("__orig_bases__", cls.__bases__)


def is_protocol(tp: type, /) -> bool:
    return (
        isinstance(tp, type)
        and bool(getattr(tp, "_is_protocol", False))
        and tp is not Protocol
    )


def get_protocol_members(tp: type, /) -> frozenset:
    if not is_protocol(tp):
        raise TypeError(f"{tp!r} is not a Protocol")
    attrs = getattr(tp, "__protocol_attrs__", None)
    if attrs is not None:
        return frozenset(attrs)
    members: set[str] = set()
    for base in tp.__mro__:
        if base.__name__ in ("Protocol", "Generic", "object"):
            continue
        if not getattr(base, "_is_protocol", False):
            continue
        for name in base.__dict__:
            if name.startswith("__") and name.endswith("__"):
                continue
            members.add(name)
    return frozenset(members)


_typing_get_origin = _typing_attr("get_origin")
_typing_get_args = _typing_attr("get_args")


def get_origin(tp: object) -> object:
    origin = _typing_get_origin(tp)
    if origin is None and hasattr(tp, "__origin__"):
        return getattr(tp, "__origin__")
    return origin


def get_args(tp: object) -> tuple:
    args = _typing_get_args(tp)
    if not args and hasattr(tp, "__args__"):
        raw = getattr(tp, "__args__")
        if isinstance(raw, tuple):
            return raw
    return args


def get_annotations(
    obj: object,
    *,
    globals: dict | None = None,
    locals: dict | None = None,
    eval_str: bool = False,
    format: object = None,
) -> dict:
    fmt_int = 1
    if format is not None:
        fmt_int = int(format)
    if fmt_int == Format.VALUE_WITH_FAKE_GLOBALS:
        raise ValueError("The VALUE_WITH_FAKE_GLOBALS format is for internal use only")
    if eval_str and fmt_int != Format.VALUE:
        raise ValueError("eval_str=True is only supported with format=Format.VALUE")

    if isinstance(obj, type):
        annotations = dict(obj.__dict__.get("__annotations__", {}))
        if globals is None:
            module_name = getattr(obj, "__module__", None)
            if isinstance(module_name, str) and module_name in _sys.modules:
                globals = _sys.modules[module_name].__dict__
        if locals is None:
            locals = dict(vars(obj))
    elif isinstance(obj, _types.ModuleType):
        annotations = dict(getattr(obj, "__annotations__", {}))
        if globals is None:
            globals = getattr(obj, "__dict__", {})
        if locals is None:
            locals = {}
    elif callable(obj):
        annotations = dict(getattr(obj, "__annotations__", {}))
        if globals is None:
            globals = getattr(obj, "__globals__", {})
        if locals is None:
            locals = {}
    else:
        raise TypeError(
            f"get_annotations() does not support {type(obj).__name__!r} objects"
        )

    if fmt_int == Format.STRING:
        return {
            key: type_repr(value) if not isinstance(value, str) else value
            for key, value in annotations.items()
        }
    if not eval_str:
        return dict(annotations)

    globals = {} if globals is None else globals
    locals = {} if locals is None else locals
    resolved: dict = {}
    for key, value in annotations.items():
        if isinstance(value, str):
            if value in locals:
                resolved[key] = locals[value]
            elif value in globals:
                resolved[key] = globals[value]
            else:
                resolved[key] = getattr(_builtins, value, value)
        else:
            resolved[key] = value
    return resolved


def evaluate_forward_ref(
    forward_ref: object,
    *,
    owner: object = None,
    globals: dict | None = None,
    locals: dict | None = None,
    type_params: object = None,
    format: object = None,
    _recursive_guard: frozenset = frozenset(),
) -> object:
    if not isinstance(forward_ref, ForwardRef):
        raise TypeError(
            "evaluate_forward_ref() requires a ForwardRef, "
            f"not {type(forward_ref).__name__!r}"
        )
    expr = getattr(forward_ref, "__forward_arg__", "")
    if expr in _recursive_guard:
        return forward_ref
    if globals is None and owner is not None:
        module_name = getattr(owner, "__module__", None)
        if isinstance(module_name, str) and module_name in _sys.modules:
            globals = _sys.modules[module_name].__dict__
    globals = {} if globals is None else globals
    locals = {} if locals is None else locals
    if expr in locals:
        return locals[expr]
    if expr in globals:
        return globals[expr]
    current = getattr(_builtins, expr, _MISSING)
    if current is not _MISSING:
        return current
    raise NameError(f"name {expr!r} is not defined")


__all__ = [
    "Annotated",
    "Any",
    "AnyStr",
    "AsyncContextManager",
    "AsyncGenerator",
    "AsyncIterable",
    "AsyncIterator",
    "Awaitable",
    "BinaryIO",
    "Callable",
    "ChainMap",
    "ClassVar",
    "Collection",
    "Concatenate",
    "Container",
    "ContextManager",
    "Coroutine",
    "Counter",
    "DefaultDict",
    "Deque",
    "Dict",
    "Final",
    "ForwardRef",
    "FrozenSet",
    "Generator",
    "Generic",
    "Hashable",
    "IO",
    "ItemsView",
    "Iterable",
    "Iterator",
    "KeysView",
    "List",
    "Literal",
    "LiteralString",
    "Mapping",
    "MappingView",
    "Match",
    "MutableMapping",
    "MutableSequence",
    "MutableSet",
    "NamedTuple",
    "Never",
    "NewType",
    "NoReturn",
    "NotRequired",
    "Optional",
    "OrderedDict",
    "ParamSpec",
    "ParamSpecArgs",
    "ParamSpecKwargs",
    "Pattern",
    "Protocol",
    "ReadOnly",
    "Required",
    "Reversible",
    "Self",
    "Sequence",
    "Set",
    "Sized",
    "SupportsAbs",
    "SupportsBytes",
    "SupportsComplex",
    "SupportsFloat",
    "SupportsIndex",
    "SupportsInt",
    "SupportsRound",
    "TYPE_CHECKING",
    "Text",
    "TextIO",
    "Tuple",
    "Type",
    "TypeAlias",
    "TypeAliasType",
    "TypeGuard",
    "TypeIs",
    "TypeVar",
    "TypeVarTuple",
    "TypedDict",
    "Union",
    "Unpack",
    "ValuesView",
    "assert_never",
    "assert_type",
    "cast",
    "clear_overloads",
    "dataclass_transform",
    "deprecated",
    "final",
    "get_args",
    "get_origin",
    "get_overloads",
    "get_type_hints",
    "is_typeddict",
    "no_type_check",
    "no_type_check_decorator",
    "overload",
    "override",
    "reveal_type",
    "runtime",
    "runtime_checkable",
    "AbstractSet",
    "Buffer",
    "CapsuleType",
    "Doc",
    "Format",
    "IntVar",
    "NoDefault",
    "NoExtraItems",
    "Reader",
    "Sentinel",
    "TypeForm",
    "Writer",
    "disjoint_base",
    "evaluate_forward_ref",
    "get_annotations",
    "get_original_bases",
    "get_protocol_members",
    "is_protocol",
    "type_repr",
]

globals().pop("_require_intrinsic", None)
