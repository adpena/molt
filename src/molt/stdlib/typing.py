"""Deterministic typing helpers for Molt.

This module tracks CPython's runtime-facing typing behavior for common helpers,
while keeping implementation small, explicit, and deterministic.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys
import builtins as _builtins
from types import ModuleType


def _typing_cast(_tp: object, value: object) -> object:
    return value


try:
    _typing_cast = _require_intrinsic("molt_typing_cast")
except RuntimeError:
    pass  # fallback to pure-Python identity above

try:
    _typing_get_origin = _require_intrinsic("molt_typing_get_origin")
except RuntimeError:
    _typing_get_origin = None

try:
    _typing_get_args = _require_intrinsic("molt_typing_get_args")
except RuntimeError:
    _typing_get_args = None

_require_intrinsic("molt_stdlib_probe")
_MOLT_GENERIC_ALIAS_NEW = _require_intrinsic("molt_generic_alias_new")
_MOLT_TYPING_TYPE_PARAM = _require_intrinsic("molt_typing_type_param")


TYPE_CHECKING = False

__all__ = [
    "Annotated",
    "Any",
    "Awaitable",
    "Callable",
    "ContextManager",
    "ClassVar",
    "Concatenate",
    "Final",
    "ForwardRef",
    "Generic",
    "IO",
    "Iterable",
    "Iterator",
    "Literal",
    "Never",
    "MutableMapping",
    "NamedTuple",
    "NewType",
    "NoReturn",
    "Optional",
    "ParamSpec",
    "Protocol",
    "Required",
    "Self",
    "Set",
    "FrozenSet",
    "List",
    "Dict",
    "Tuple",
    "SupportsIndex",
    "SupportsInt",
    "Text",
    "TextIO",
    "BinaryIO",
    "TYPE_CHECKING",
    "TypeAlias",
    "TypeGuard",
    "TypeVar",
    "TypeVarTuple",
    "TypedDict",
    "Union",
    "NotRequired",
    "cast",
    "get_args",
    "get_origin",
    "get_type_hints",
    "override",
    "overload",
    "runtime_checkable",
    "deprecated",
    "assert_type",
    "assert_never",
    "is_typeddict",
    "LiteralString",
    "get_overloads",
    "clear_overloads",
    "dataclass_transform",
]

_NoneType = type(None)
try:
    _UnionType = type(int | str)
except Exception:
    _UnionType = None

_SUPPORTS_TYPEVAR_DEFAULTS = tuple(_sys.version_info[:2]) >= (3, 13)
_TYPEVAR_NO_DEFAULT = object()

if _SUPPORTS_TYPEVAR_DEFAULTS:

    class _NoDefaultType:
        __slots__ = ()

        def __repr__(self) -> str:
            return "typing.NoDefault"

    NoDefault = _NoDefaultType()
    __all__.append("NoDefault")
else:
    NoDefault = _TYPEVAR_NO_DEFAULT


class _AnnotatedOrigin:
    pass


_AnnotatedOrigin.__module__ = "typing"
_AnnotatedOrigin.__name__ = "Annotated"
_AnnotatedOrigin.__qualname__ = "Annotated"


def _as_tuple(params: object) -> tuple[object, ...]:
    if isinstance(params, tuple):
        return params
    return (params,)


def _type_repr(arg: object) -> str:
    if isinstance(arg, _TypingBase):
        return repr(arg)
    if arg is Ellipsis:
        return "..."
    if arg is None:
        return "None"
    if arg is _NoneType:
        return "NoneType"
    if isinstance(arg, str):
        return repr(arg)
    name = getattr(arg, "__qualname__", None)
    if isinstance(name, str) and name:
        return name
    name = getattr(arg, "__name__", None)
    if isinstance(name, str) and name:
        return name
    return repr(arg)


def _format_args(args: tuple[object, ...]) -> str:
    return ", ".join(_type_repr(arg) for arg in args)


class _TypingBase:
    __slots__ = ()

    def __or__(self, other: object) -> object:
        return _make_union((self, other))

    def __ror__(self, other: object) -> object:
        return _make_union((other, self))


class _SpecialForm(_TypingBase):
    __slots__ = ("_name", "_getitem")

    def __init__(self, name: str, getitem=None) -> None:
        self._name = name
        self._getitem = getitem

    def __repr__(self) -> str:
        return f"typing.{self._name}"

    def __getitem__(self, params: object) -> object:
        if self._getitem is None:
            raise TypeError(f"{self!r} is not subscriptable")
        return self._getitem(params)

    def __call__(self, *_args: object, **_kwargs: object) -> object:
        raise TypeError(f"Cannot instantiate {self!r}")


class _SpecialGenericAlias(_TypingBase):
    __slots__ = ("__origin__", "_name")

    def __init__(self, origin: object, name: str) -> None:
        self.__origin__ = origin
        self._name = name

    def __repr__(self) -> str:
        return f"typing.{self._name}"

    def __getitem__(self, params: object) -> "_GenericAlias":
        return _GenericAlias(self.__origin__, params, self._name)

    def __call__(self, *_args: object, **_kwargs: object) -> object:
        raise TypeError(f"Cannot instantiate {self!r}")


class _LazySpecialGenericAlias(_TypingBase):
    __slots__ = ("_origin_name", "_name", "_origin_cache")

    def __init__(self, origin_name: str, name: str) -> None:
        self._origin_name = origin_name
        self._name = name
        self._origin_cache = None

    def _origin(self) -> object:
        if self._origin_cache is None:
            abc_mod = _load_collections_abc()
            origin = getattr(abc_mod, self._origin_name, None)
            if origin is None:
                raise RuntimeError(
                    f"typing missing _collections_abc.{self._origin_name}"
                )
            self._origin_cache = origin
        return self._origin_cache

    @property
    def __origin__(self) -> object:
        return self._origin()

    def __repr__(self) -> str:
        return f"typing.{self._name}"

    def __getitem__(self, params: object) -> "_GenericAlias":
        return _GenericAlias(self._origin(), params, self._name)

    def __call__(self, *_args: object, **_kwargs: object) -> object:
        raise TypeError(f"Cannot instantiate {self!r}")


class _GenericAlias(_TypingBase):
    __slots__ = ("__origin__", "__args__", "_name")

    def __init__(self, origin: object, args: object, name: str | None = None) -> None:
        self.__origin__ = origin
        self.__args__ = _as_tuple(args)
        self._name = name

    def __repr__(self) -> str:
        name = self._name
        if name:
            base = f"typing.{name}"
        else:
            base = _type_repr(self.__origin__)
        return f"{base}[{_format_args(self.__args__)}]"

    def __mro_entries__(self, _bases: tuple[object, ...]) -> tuple[object, ...]:
        origin = self.__origin__
        if isinstance(origin, type):
            return (origin,)
        return ()


class _MoltTypeAlias(_TypingBase):
    __slots__ = ("__name__", "__value__", "__type_params__", "__parameters__")

    def __init__(
        self, name: str, value: object, type_params: tuple[object, ...]
    ) -> None:
        self.__name__ = name
        self.__value__ = value
        self.__type_params__ = type_params
        self.__parameters__ = type_params

    def __repr__(self) -> str:
        return self.__name__

    def __getitem__(self, params: object) -> "_MoltTypeAliasApplied":
        args = _as_tuple(params)
        return _MoltTypeAliasApplied(self, args)


class _MoltTypeAliasApplied(_TypingBase):
    __slots__ = ("_alias", "__args__", "__origin__", "__value__", "__parameters__")

    def __init__(self, alias: _MoltTypeAlias, args: tuple[object, ...]) -> None:
        self._alias = alias
        self.__args__ = args
        self.__origin__ = alias
        self.__value__ = alias.__value__
        self.__parameters__ = alias.__type_params__

    def __repr__(self) -> str:
        return f"{self._alias.__name__}[{_format_args(self.__args__)}]"


class _UnionGenericAlias(_TypingBase):
    __slots__ = ("__args__", "__origin__")

    def __init__(self, args: tuple[object, ...]) -> None:
        self.__args__ = args
        self.__origin__ = Union

    def __repr__(self) -> str:
        args = self.__args__
        if len(args) == 2 and _NoneType in args:
            other = args[0] if args[1] is _NoneType else args[1]
            return f"typing.Optional[{_type_repr(other)}]"
        return f"typing.Union[{_format_args(args)}]"

    def __call__(self, *_args: object, **_kwargs: object) -> object:
        raise TypeError(f"Cannot instantiate {self!r}")


_UnionAlias = _UnionGenericAlias


class _LiteralAlias(_TypingBase):
    __slots__ = ("__args__", "__origin__")

    def __init__(self, args: tuple[object, ...]) -> None:
        self.__args__ = args
        self.__origin__ = Literal

    def __repr__(self) -> str:
        return f"typing.Literal[{_format_args(self.__args__)}]"


class _AnnotatedAlias(_TypingBase):
    __slots__ = ("__origin__", "__args__", "__metadata__")

    def __init__(self, origin: object, metadata: tuple[object, ...]) -> None:
        self.__origin__ = origin
        self.__args__ = (origin,)
        self.__metadata__ = metadata

    def __repr__(self) -> str:
        args = (self.__origin__,) + self.__metadata__
        return f"typing.Annotated[{_format_args(args)}]"


class _ConcatenateAlias(_TypingBase):
    __slots__ = ("__args__", "__origin__")

    def __init__(self, args: tuple[object, ...]) -> None:
        self.__args__ = args
        self.__origin__ = Concatenate

    def __repr__(self) -> str:
        return f"typing.Concatenate[{_format_args(self.__args__)}]"


class _CallableAlias(_TypingBase):
    __slots__ = ("__args__", "__origin__", "_arglist", "_return")

    def __init__(self, arglist: object, ret: object) -> None:
        self.__origin__ = getattr(_load_collections_abc(), "Callable")
        self._arglist = arglist
        self._return = ret
        if arglist is Ellipsis:
            self.__args__ = (Ellipsis, ret)
        elif isinstance(arglist, _ConcatenateAlias):
            self.__args__ = (arglist, ret)
        else:
            args = _as_tuple(arglist)
            self.__args__ = args + (ret,)

    def __repr__(self) -> str:
        if self._arglist is Ellipsis:
            args = (Ellipsis, self._return)
        elif isinstance(self._arglist, _ConcatenateAlias):
            args = (self._arglist, self._return)
        else:
            args = (list(_as_tuple(self._arglist)), self._return)
        return f"typing.Callable[{_format_args(args)}]"


def _normalize_union_args(args: tuple[object, ...]) -> tuple[object, ...]:
    flat: list[object] = []
    for arg in args:
        if arg is None:
            arg = _NoneType
        if isinstance(arg, _UnionAlias):
            flat.extend(arg.__args__)
            continue
        if _UnionType is not None and isinstance(arg, _UnionType):
            flat.extend(getattr(arg, "__args__", ()))
            continue
        flat.append(arg)
    unique: list[object] = []
    for arg in flat:
        if not any(arg is seen for seen in unique):
            unique.append(arg)
    return tuple(unique)


def _make_union(args: object) -> object:
    norm = _normalize_union_args(_as_tuple(args))
    if not norm:
        raise TypeError("Union requires at least one argument")
    if len(norm) == 1:
        return norm[0]
    return _UnionGenericAlias(norm)


def _make_optional(arg: object) -> object:
    return _make_union((arg, _NoneType))


def _make_literal(params: object) -> _LiteralAlias:
    return _LiteralAlias(_as_tuple(params))


def _make_annotated(params: object) -> _AnnotatedAlias:
    args = _as_tuple(params)
    if len(args) < 2:
        raise TypeError("Annotated requires at least two arguments")
    return _AnnotatedAlias(args[0], args[1:])


def _make_concatenate(params: object) -> _ConcatenateAlias:
    args = _as_tuple(params)
    if not args:
        raise TypeError("Concatenate requires at least one argument")
    return _ConcatenateAlias(args)


def _make_callable(params: object) -> _CallableAlias:
    args = _as_tuple(params)
    if len(args) != 2:
        raise TypeError("Callable must be used as Callable[[args], return]")
    arglist, ret = args
    return _CallableAlias(arglist, ret)


class _AnyMeta(type):
    def __repr__(cls) -> str:
        return "typing.Any"

    def __call__(cls, *_args: object, **_kwargs: object) -> object:
        raise TypeError("Cannot instantiate typing.Any")

    def __getitem__(cls, _params: object) -> object:
        return cls

    def __or__(cls, other: object) -> object:
        return _make_union((cls, other))

    def __ror__(cls, other: object) -> object:
        return _make_union((other, cls))


class Any(metaclass=_AnyMeta):
    pass


Any.__module__ = __name__

Union = _SpecialForm("Union", _make_union)
Optional = _SpecialForm("Optional", _make_optional)
Literal = _SpecialForm("Literal", _make_literal)
Annotated = _SpecialForm("Annotated", _make_annotated)
ClassVar = _SpecialForm(
    "ClassVar", lambda params: _GenericAlias(ClassVar, params, "ClassVar")
)
Final = _SpecialForm("Final", lambda params: _GenericAlias(Final, params, "Final"))
Concatenate = _SpecialForm("Concatenate", _make_concatenate)
Callable = _SpecialForm("Callable", _make_callable)
# Keep runtime shape aligned (type: _SpecialGenericAlias) without importing
# contextlib at typing import time (avoids contextlib<->typing cycle).
ContextManager = _SpecialGenericAlias(object, "ContextManager")
Self = _SpecialForm("Self")
IO = _SpecialForm("IO", lambda params: _GenericAlias(IO, params, "IO"))


class BinaryIO:
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "BinaryIO")


class TextIO:
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "TextIO")


BinaryIO.__module__ = __name__
TextIO.__module__ = __name__
Text = str

Never = _SpecialForm("Never")
NoReturn = _SpecialForm("NoReturn")
TypeAlias = _SpecialForm("TypeAlias")
TypeGuard = _SpecialForm(
    "TypeGuard", lambda params: _GenericAlias(TypeGuard, params, "TypeGuard")
)
Required = _SpecialForm(
    "Required", lambda params: _GenericAlias(Required, params, "Required")
)
NotRequired = _SpecialForm(
    "NotRequired", lambda params: _GenericAlias(NotRequired, params, "NotRequired")
)


def _require_importlib_util_module() -> object:
    modules = getattr(_sys, "modules", {})
    mod = modules.get("importlib.util")
    if mod is None:
        mod = __import__("importlib.util", fromlist=("find_spec", "module_from_spec"))
    if mod is None:
        raise RuntimeError("typing requires importlib.util")
    return mod


def _load_collections_abc() -> ModuleType:
    cached = globals().get("_ABC_CACHE")
    if isinstance(cached, ModuleType):
        return cached
    import _collections_abc as abc_mod_raw

    abc_mod = _typing_cast(ModuleType, abc_mod_raw)
    if getattr(abc_mod, "__name__", None) == "_abc":
        raise RuntimeError("typing requires _collections_abc, not _abc")
    required_names = ("Awaitable", "Iterable", "Iterator", "MutableMapping", "Callable")
    missing = [name for name in required_names if getattr(abc_mod, name, None) is None]
    if missing:
        repaired = _reload_collections_abc()
        if isinstance(repaired, ModuleType):
            abc_mod = repaired
            missing = [
                name for name in required_names if getattr(abc_mod, name, None) is None
            ]
    if missing:
        raise RuntimeError(f"typing missing _collections_abc.{missing[0]}")
    globals()["_ABC_CACHE"] = abc_mod
    return abc_mod


def _reload_collections_abc() -> ModuleType | None:
    modules = getattr(_sys, "modules", {})
    previous = modules.pop("_collections_abc", None)
    importlib_util = _require_importlib_util_module()
    find_spec = getattr(importlib_util, "find_spec", None)
    module_from_spec = getattr(importlib_util, "module_from_spec", None)
    if not callable(find_spec) or not callable(module_from_spec):
        if previous is not None:
            modules["_collections_abc"] = previous
        return None
    try:
        spec = find_spec("_collections_abc", None)
        if spec is None:
            if previous is not None:
                modules["_collections_abc"] = previous
            return None
        module = module_from_spec(spec)
        modules["_collections_abc"] = module
        loader = getattr(spec, "loader", None)
        if loader is not None:
            if hasattr(loader, "exec_module"):
                loader.exec_module(module)
            elif hasattr(loader, "load_module"):
                loaded = loader.load_module("_collections_abc")
                if loaded is not None:
                    module = loaded
        reloaded = modules.get("_collections_abc", module)
        return _typing_cast(ModuleType, reloaded)
    except Exception:
        modules.pop("_collections_abc", None)
        if previous is not None:
            modules["_collections_abc"] = previous
        return None


Awaitable = _LazySpecialGenericAlias("Awaitable", "Awaitable")
print("[TYPING-DBG] before _load_collections_abc")
_abc_module = _load_collections_abc()
print(f"[TYPING-DBG] after _load_collections_abc: {_abc_module}")
Iterable = _SpecialGenericAlias(getattr(_abc_module, "Iterable"), "Iterable")
Iterator = _SpecialGenericAlias(getattr(_abc_module, "Iterator"), "Iterator")
MutableMapping = _LazySpecialGenericAlias("MutableMapping", "MutableMapping")

_types_mod = _sys.modules.get("types")
if _types_mod is not None:
    try:
        setattr(_types_mod, "Any", Any)
        setattr(_types_mod, "Iterable", Iterable)
    except Exception:
        pass

List = _SpecialGenericAlias(list, "List")
Dict = _SpecialGenericAlias(dict, "Dict")
Tuple = _SpecialGenericAlias(tuple, "Tuple")
Set = _SpecialGenericAlias(set, "Set")
FrozenSet = _SpecialGenericAlias(frozenset, "FrozenSet")


class _TypeVarLike(_TypingBase):
    __slots__ = (
        "__name__",
        "_covariant",
        "_contravariant",
        "_bound",
        "_constraints",
        "_default",
        "_pep695",
    )

    def __init__(
        self,
        name: str,
        covariant: bool,
        contravariant: bool,
        bound: object | None,
        constraints: tuple[object, ...],
        default: object = _TYPEVAR_NO_DEFAULT,
        pep695: bool = False,
    ) -> None:
        self.__name__ = name
        self._covariant = covariant
        self._contravariant = contravariant
        self._bound = bound
        self._constraints = constraints
        self._default = default
        self._pep695 = pep695

    @property
    def __constraints__(self) -> tuple[object, ...]:
        return self._constraints

    @property
    def __bound__(self) -> object | None:
        return self._bound

    @property
    def __covariant__(self) -> bool:
        return self._covariant

    @property
    def __contravariant__(self) -> bool:
        return self._contravariant


if _SUPPORTS_TYPEVAR_DEFAULTS:

    def _typevarlike_default(self: _TypeVarLike) -> object:
        return self._default

    def _typevarlike_has_default(self: _TypeVarLike) -> bool:
        return self._default is not NoDefault

    _TypeVarLike.__default__ = property(_typevarlike_default)  # type: ignore[attr-defined]
    _TypeVarLike.has_default = _typevarlike_has_default  # type: ignore[attr-defined]


class _TypeVar(_TypeVarLike):
    __slots__ = ()

    def __repr__(self) -> str:
        if self._pep695:
            return self.__name__
        if self._covariant:
            return f"+{self.__name__}"
        if self._contravariant:
            return f"-{self.__name__}"
        return f"~{self.__name__}"

    __str__ = __repr__


class _TypeVarTuple(_TypingBase):
    __slots__ = ("__name__",)

    def __init__(self, name: str) -> None:
        self.__name__ = name

    def __repr__(self) -> str:
        return self.__name__

    __str__ = __repr__


class _ParamSpec(_TypeVarLike):
    __slots__ = ("args", "kwargs")

    def __init__(
        self,
        name: str,
        covariant: bool,
        contravariant: bool,
        bound: object | None,
        constraints: tuple[object, ...],
    ) -> None:
        super().__init__(
            name,
            covariant=covariant,
            contravariant=contravariant,
            bound=bound,
            constraints=constraints,
        )
        self.args = _ParamSpecArgs(self)
        self.kwargs = _ParamSpecKwargs(self)

    def __repr__(self) -> str:
        return f"~{self.__name__}"

    __str__ = __repr__


class _ParamSpecArgs(_TypingBase):
    __slots__ = ("_owner",)

    def __init__(self, owner: _ParamSpec) -> None:
        self._owner = owner

    def __repr__(self) -> str:
        return f"{self._owner.__name__}.args"

    __str__ = __repr__


class _ParamSpecKwargs(_TypingBase):
    __slots__ = ("_owner",)

    def __init__(self, owner: _ParamSpec) -> None:
        self._owner = owner

    def __repr__(self) -> str:
        return f"{self._owner.__name__}.kwargs"

    __str__ = __repr__


def TypeVar(
    name: str,
    *constraints: object,
    bound: object | None = None,
    covariant: bool = False,
    contravariant: bool = False,
    default: object = _TYPEVAR_NO_DEFAULT,
) -> _TypeVar:
    if constraints and bound is not None:
        raise TypeError("TypeVar cannot have both bound and constraints")
    if default is not _TYPEVAR_NO_DEFAULT and not _SUPPORTS_TYPEVAR_DEFAULTS:
        raise TypeError("'default' is an invalid keyword argument for typevar()")
    resolved_default = default
    if _SUPPORTS_TYPEVAR_DEFAULTS and default is _TYPEVAR_NO_DEFAULT:
        resolved_default = NoDefault
    return _TypeVar(
        name=name,
        covariant=covariant,
        contravariant=contravariant,
        bound=bound,
        constraints=tuple(constraints),
        default=resolved_default,
        pep695=False,
    )


def ParamSpec(
    name: str,
    *constraints: object,
    bound: object | None = None,
    covariant: bool = False,
    contravariant: bool = False,
) -> _ParamSpec:
    if constraints and bound is not None:
        raise TypeError("ParamSpec cannot have both bound and constraints")
    return _ParamSpec(
        name=name,
        covariant=covariant,
        contravariant=contravariant,
        bound=bound,
        constraints=tuple(constraints),
    )


def TypeVarTuple(name: str) -> _TypeVarTuple:
    return _TypeVarTuple(name)


def _molt_type_param(name: str, default: object = _TYPEVAR_NO_DEFAULT) -> _TypeVar:
    if default is _TYPEVAR_NO_DEFAULT:
        return _typing_cast(_TypeVar, _MOLT_TYPING_TYPE_PARAM(TypeVar, name))
    if not _SUPPORTS_TYPEVAR_DEFAULTS:
        raise TypeError("'default' is an invalid keyword argument for typevar()")

    def _typevar_ctor_with_default(param_name: str) -> _TypeVar:
        return TypeVar(param_name, default=default)

    return _typing_cast(
        _TypeVar, _MOLT_TYPING_TYPE_PARAM(_typevar_ctor_with_default, name)
    )


def _molt_class_getitem(cls: object, params: object) -> object:
    args = _as_tuple(params)
    runtime_args = args[0] if len(args) == 1 else args
    return _MOLT_GENERIC_ALIAS_NEW(cls, runtime_args)


def _molt_type_alias(
    name: str, value: object, type_params: tuple[object, ...]
) -> _MoltTypeAlias:
    params = _as_tuple(type_params)
    return _MoltTypeAlias(name, value, params)


class Generic:
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "Generic")


_MOLT_PROTOCOL_CHECK = _require_intrinsic("molt_protocol_check")
_MOLT_PROTOCOL_GET_STRUCTURAL_MEMBERS = _require_intrinsic(
    "molt_protocol_get_structural_members"
)
_MOLT_PROTOCOL_REGISTER = _require_intrinsic("molt_protocol_register")


class _ProtocolMeta(type):
    """Metaclass for Protocol classes.

    Protocol member collection is delegated to the Rust intrinsic
    ``molt_protocol_get_structural_members`` and structural isinstance /
    issubclass checks are performed by ``molt_protocol_check``.  This
    eliminates the previous Python-level ABC scaffolding in favour of
    deterministic, AOT-friendly intrinsic paths.
    """

    def __init__(cls, name, bases, namespace, **kwargs) -> None:
        super().__init__(name, bases, namespace)
        global _PROTOCOL_BASE
        if cls.__name__ == "Protocol" and cls.__module__ == __name__:
            _PROTOCOL_BASE = cls
            cls._is_protocol = True
            cls._is_runtime_protocol = False
            cls.__protocol_attrs__ = frozenset()
            return
        is_protocol = any(getattr(base, "_is_protocol", False) for base in bases)
        cls._is_protocol = is_protocol
        if not is_protocol:
            return
        # Delegate structural member extraction to the Rust intrinsic.
        cls.__protocol_attrs__ = _MOLT_PROTOCOL_GET_STRUCTURAL_MEMBERS(cls)
        cls._is_runtime_protocol = False

    def __instancecheck__(cls, instance) -> bool:
        return _MOLT_PROTOCOL_CHECK(cls, instance)

    def __subclasscheck__(cls, subclass) -> bool:
        return _MOLT_PROTOCOL_CHECK(cls, subclass)


class Protocol(metaclass=_ProtocolMeta):
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "Protocol")


_PROTOCOL_BASE: type | None = None


def runtime_checkable(cls):
    if not getattr(cls, "_is_protocol", False):
        raise TypeError("@runtime_checkable can only be applied to protocol classes")
    cls._is_runtime_protocol = True
    _MOLT_PROTOCOL_REGISTER(cls, cls)
    return cls


@runtime_checkable
class SupportsIndex(Protocol):
    def __index__(self) -> int: ...


@runtime_checkable
class SupportsInt(Protocol):
    def __int__(self) -> int: ...


class NamedTuple(tuple):
    __slots__ = ()
    _fields: tuple[str, ...] = ()
    _field_defaults: dict[str, object] = {}

    def __init_subclass__(cls, **kwargs) -> None:
        super().__init_subclass__(**kwargs)
        if cls is NamedTuple:
            return
        annotations = dict(getattr(cls, "__annotations__", {}))
        fields = tuple(annotations.keys())
        defaults: dict[str, object] = {}
        for name in fields:
            if hasattr(cls, name):
                defaults[name] = getattr(cls, name)
        cls._fields = fields
        cls._field_defaults = defaults

        def __new__(subcls, *values: object, **kw: object):
            if kw:
                values_list = list(values)
                for name in fields[len(values_list) :]:
                    if name in kw:
                        values_list.append(kw.pop(name))
                if kw:
                    unexpected = next(iter(kw))
                    raise TypeError(
                        f"{subcls.__name__} got an unexpected keyword argument {unexpected!r}"
                    )
                values = tuple(values_list)
            if len(values) < len(fields):
                filled = list(values)
                for name in fields[len(filled) :]:
                    if name in defaults:
                        filled.append(defaults[name])
                    else:
                        raise TypeError(
                            f"{subcls.__name__} expected {len(fields)} arguments, got {len(values)}"
                        )
                values = tuple(filled)
            elif len(values) > len(fields):
                raise TypeError(
                    f"{subcls.__name__} expected {len(fields)} arguments, got {len(values)}"
                )
            return tuple.__new__(subcls, values)

        cls.__new__ = __new__  # type: ignore[assignment]

        for index, name in enumerate(fields):

            def _getter(self, i=index):
                return self[i]

            setattr(cls, name, property(_getter))

    def __repr__(self) -> str:
        cls_name = type(self).__name__
        parts = []
        for name, value in zip(type(self)._fields, self):
            parts.append(f"{name}={value!r}")
        inner = ", ".join(parts)
        return f"{cls_name}({inner})"


class ForwardRef(_TypingBase):
    __slots__ = ("__forward_arg__", "__forward_module__")

    def __init__(self, arg: str, module: str | None = None) -> None:
        self.__forward_arg__ = arg
        self.__forward_module__ = module

    def __repr__(self) -> str:
        return f"ForwardRef({self.__forward_arg__!r})"


class _TypedDictMeta(type):
    def __new__(mcls, name, bases, namespace, total=True, **kwargs):
        annotations = dict(namespace.get("__annotations__", {}))
        required = set(annotations.keys()) if total else set()
        optional = set() if total else set(annotations.keys())
        namespace["__annotations__"] = annotations
        cls = super().__new__(mcls, name, bases, namespace)
        cls.__required_keys__ = frozenset(required)
        cls.__optional_keys__ = frozenset(optional)
        cls.__total__ = bool(total)
        return cls


class TypedDict(dict, metaclass=_TypedDictMeta):
    pass


def NewType(name: str, tp: object):
    def _new(value):
        return value

    _new.__name__ = name
    _new.__qualname__ = name
    _new.__module__ = _sys._getframe(1).f_globals.get("__name__", "__main__")
    setattr(_new, "__supertype__", tp)
    return _new


def cast(_typ: object, value: object) -> object:
    return _typing_cast(_typ, value)


def overload(func):
    return func


def assert_type(val, tp, /):
    return val


def assert_never(arg, /):
    raise AssertionError("Expected code to be unreachable")


def is_typeddict(tp, /):
    return (
        isinstance(tp, type) and issubclass(tp, dict) and hasattr(tp, "__annotations__")
    )


LiteralString = _SpecialForm("LiteralString")


def get_overloads(func):
    return getattr(func, "__overloads__", [])


def clear_overloads():
    pass


def dataclass_transform(**kwargs):
    def decorator(cls_or_fn):
        cls_or_fn.__dataclass_transform__ = kwargs
        return cls_or_fn

    return decorator


def override(method):
    """Indicate that a method is intended to override a method in a base class.

    PEP 698 -- added in Python 3.12.
    """
    try:
        method.__override__ = True
    except (AttributeError, TypeError):
        pass
    return method


def _load_deprecated():
    """Lazy-load ``warnings.deprecated`` to avoid importing warnings at typing
    import time (prevents circular import pressure)."""
    import warnings as _warnings_mod

    return _warnings_mod.deprecated


class deprecated:
    """Indicate that a class, function or overload is deprecated.

    PEP 702 -- added in Python 3.13.  Re-exported here for convenience;
    the canonical implementation lives in ``warnings.deprecated``.

    Usage::

        @deprecated("Use new_func instead")
        def old_func():
            ...

        @deprecated("Use NewClass instead")
        class OldClass:
            ...
    """

    def __init__(self, message, /, *, category=DeprecationWarning, stacklevel=1):
        self._impl = _load_deprecated()(
            message, category=category, stacklevel=stacklevel
        )
        self.message = self._impl.message
        self.category = self._impl.category
        self.stacklevel = self._impl.stacklevel

    def __call__(self, arg):
        return self._impl(arg)


def get_origin(tp: object) -> object | None:
    if isinstance(tp, _AnnotatedAlias):
        return _AnnotatedOrigin
    if isinstance(tp, _LiteralAlias):
        return Literal
    if isinstance(tp, _UnionAlias):
        return Union
    if isinstance(tp, _ConcatenateAlias):
        return Concatenate
    if isinstance(tp, _CallableAlias):
        return getattr(_load_collections_abc(), "Callable")
    if _UnionType is not None and isinstance(tp, _UnionType):
        return _UnionType
    return getattr(tp, "__origin__", None)


def get_args(tp: object) -> tuple[object, ...]:
    if isinstance(tp, _AnnotatedAlias):
        return (tp.__origin__,) + tp.__metadata__
    if isinstance(tp, _LiteralAlias):
        return tp.__args__
    if isinstance(tp, _UnionAlias):
        return tp.__args__
    if isinstance(tp, _ConcatenateAlias):
        return tp.__args__
    if isinstance(tp, _CallableAlias):
        if tp._arglist is Ellipsis:
            return (Ellipsis, tp._return)
        if isinstance(tp._arglist, _ConcatenateAlias):
            return (tp._arglist, tp._return)
        return (list(_as_tuple(tp._arglist)), tp._return)
    args = getattr(tp, "__args__", ())
    if isinstance(args, tuple):
        return args
    return ()


def _typing_lookup_name(expr: str, globalns: dict, localns: dict) -> object:
    if expr in localns:
        return localns[expr]
    if expr in globalns:
        return globalns[expr]
    if hasattr(_builtins, expr):
        return getattr(_builtins, expr)
    raise NameError(f"name '{expr}' is not defined")


def _typing_split_top_level(expr: str, sep: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    start = 0
    for idx, ch in enumerate(expr):
        if ch in "([":
            depth += 1
        elif ch in ")]":
            if depth > 0:
                depth -= 1
        elif ch == sep and depth == 0:
            piece = expr[start:idx].strip()
            if piece:
                parts.append(piece)
            start = idx + 1
    tail = expr[start:].strip()
    if tail:
        parts.append(tail)
    return parts


def _typing_strip_wrapping_parens(expr: str) -> str:
    text = expr.strip()
    while text.startswith("(") and text.endswith(")"):
        depth = 0
        balanced = True
        for idx, ch in enumerate(text):
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0 and idx != len(text) - 1:
                    balanced = False
                    break
        if not balanced or depth != 0:
            break
        text = text[1:-1].strip()
    return text


def _typing_parse_subscription(expr: str) -> tuple[str, str] | None:
    text = expr.strip()
    if not text.endswith("]"):
        return None
    depth = 0
    open_idx = -1
    for idx, ch in enumerate(text):
        if ch == "[":
            if depth == 0:
                open_idx = idx
            depth += 1
        elif ch == "]":
            depth -= 1
            if depth == 0:
                if idx != len(text) - 1 or open_idx <= 0:
                    return None
                return (text[:open_idx].strip(), text[open_idx + 1 : -1].strip())
            if depth < 0:
                return None
    return None


def _typing_eval_annotation_expr(expr: str, globalns: dict, localns: dict) -> object:
    text = _typing_strip_wrapping_parens(expr)
    if not text:
        raise ValueError("empty annotation expression")
    if len(text) >= 2 and text[0] == text[-1] and text[0] in {"'", '"'}:
        return _typing_eval_annotation_expr(text[1:-1], globalns, localns)

    union_parts = _typing_split_top_level(text, "|")
    if len(union_parts) > 1:
        evaluated = [
            _typing_eval_annotation_expr(part, globalns, localns)
            for part in union_parts
        ]
        out = evaluated[0]
        for part in evaluated[1:]:
            out = out | part
        return out

    sub = _typing_parse_subscription(text)
    if sub is not None:
        base_expr, args_expr = sub
        base = _typing_eval_annotation_expr(base_expr, globalns, localns)
        arg_parts = _typing_split_top_level(args_expr, ",")
        if not arg_parts:
            class_getitem = getattr(base, "__class_getitem__", None)
            if class_getitem is not None:
                return class_getitem(())
            return base[()]
        args = tuple(
            _typing_eval_annotation_expr(part, globalns, localns) for part in arg_parts
        )
        payload: object = args[0] if len(args) == 1 else args
        if base in {
            _builtins.list,
            _builtins.dict,
            _builtins.tuple,
            _builtins.set,
            _builtins.frozenset,
        }:
            return _MOLT_GENERIC_ALIAS_NEW(base, payload)
        class_getitem = getattr(base, "__class_getitem__", None)
        if class_getitem is not None:
            return class_getitem(payload)
        return base[payload]

    if "." in text:
        parts = text.split(".")
        cur = _typing_lookup_name(parts[0], globalns, localns)
        for part in parts[1:]:
            cur = getattr(cur, part)
        return cur
    return _typing_lookup_name(text, globalns, localns)


def _eval_type(value: object, globalns: dict, localns: dict) -> object:
    if isinstance(value, ForwardRef):
        expr = value.__forward_arg__
        try:
            return _typing_eval_annotation_expr(expr, globalns, localns)
        except Exception:
            return eval(expr, globalns, localns)
    if isinstance(value, str):
        try:
            return _typing_eval_annotation_expr(value, globalns, localns)
        except Exception:
            return eval(value, globalns, localns)
    return value


def get_type_hints(
    obj: object,
    globalns: dict | None = None,
    localns: dict | None = None,
    include_extras: bool = False,
) -> dict[str, object]:
    annotations = getattr(obj, "__annotations__", None)
    if not annotations:
        return {}
    if globalns is None:
        if hasattr(obj, "__globals__"):
            globalns = obj.__globals__  # type: ignore[assignment]
        else:
            module = getattr(obj, "__module__", None)
            if module and module in _sys.modules:
                globalns = _sys.modules[module].__dict__
            else:
                globalns = {}
    if localns is None:
        if isinstance(obj, type):
            localns = dict(vars(obj))
        else:
            localns = {}
    assert globalns is not None
    assert localns is not None
    hints: dict[str, object] = {}
    for name, value in dict(annotations).items():
        evaluated = _eval_type(value, globalns, localns)
        if not include_extras and isinstance(evaluated, _AnnotatedAlias):
            evaluated = evaluated.__origin__
        hints[name] = evaluated
    return hints

globals().pop("_require_intrinsic", None)
