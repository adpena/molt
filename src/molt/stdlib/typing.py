"""Deterministic typing helpers for Molt.

This module tracks CPython's runtime-facing typing behavior for common helpers,
while keeping implementation small, explicit, and deterministic.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys
from types import ModuleType


def _typing_cast(_tp: object, value: object) -> object:
    return value


_require_intrinsic("molt_stdlib_probe", globals())


def _install_fallback_abc() -> ModuleType:
    class _FallbackABC:
        __slots__ = ()

    class _Iterable(_FallbackABC):
        pass

    class _Iterator(_Iterable):
        pass

    class _MutableMapping(_FallbackABC):
        pass

    class _Callable(_FallbackABC):
        pass

    fallback = ModuleType("_molt_fallback_abc")
    setattr(fallback, "Iterable", _Iterable)
    setattr(fallback, "Iterator", _Iterator)
    setattr(fallback, "MutableMapping", _MutableMapping)
    setattr(fallback, "Callable", _Callable)
    return fallback


_abc_mod: ModuleType | None
_abc: ModuleType
try:
    import _collections_abc as _abc_mod_raw
except Exception:
    _abc_mod = None
else:
    _abc_mod = _typing_cast(ModuleType, _abc_mod_raw)

if _abc_mod is None:
    _abc_mod = _install_fallback_abc()
else:
    _abc_iterable = getattr(_abc_mod, "Iterable", None)
    if _abc_iterable is None:
        _abc_mod = _install_fallback_abc()
    elif getattr(_abc_mod, "__name__", None) == "_abc":
        _abc_mod = _install_fallback_abc()

_abc = _abc_mod

TYPE_CHECKING = False

__all__ = [
    "Annotated",
    "Any",
    "Callable",
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
    "overload",
    "runtime_checkable",
]

_NoneType = type(None)
try:
    _UnionType = type(int | str)
except Exception:
    _UnionType = None


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


class _SpecialGenericAlias(_TypingBase):
    __slots__ = ("__origin__", "_name")

    def __init__(self, origin: object, name: str) -> None:
        self.__origin__ = origin
        self._name = name

    def __repr__(self) -> str:
        return f"typing.{self._name}"

    def __getitem__(self, params: object) -> "_GenericAlias":
        return _GenericAlias(self.__origin__, params, self._name)


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


class _UnionAlias(_TypingBase):
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
        self.__origin__ = getattr(_abc, "Callable")
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
    return _UnionAlias(norm)


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


Any = _SpecialForm("Any", lambda _params: Any)
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
Self = _SpecialForm("Self")
IO = _SpecialForm("IO", lambda params: _GenericAlias(IO, params, "IO"))
BinaryIO = _SpecialForm(
    "BinaryIO", lambda params: _GenericAlias(BinaryIO, params, "BinaryIO")
)
TextIO = _SpecialForm("TextIO", lambda params: _GenericAlias(TextIO, params, "TextIO"))
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

Iterable = _SpecialGenericAlias(_abc.Iterable, "Iterable")
Iterator = _SpecialGenericAlias(_abc.Iterator, "Iterator")
MutableMapping = _SpecialGenericAlias(_abc.MutableMapping, "MutableMapping")

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
    __slots__ = ("__name__", "_covariant", "_contravariant", "_bound", "_constraints")

    def __init__(
        self,
        name: str,
        covariant: bool,
        contravariant: bool,
        bound: object | None,
        constraints: tuple[object, ...],
    ) -> None:
        self.__name__ = name
        self._covariant = covariant
        self._contravariant = contravariant
        self._bound = bound
        self._constraints = constraints

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


class _TypeVar(_TypeVarLike):
    __slots__ = ()

    def __repr__(self) -> str:
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
) -> _TypeVar:
    if constraints and bound is not None:
        raise TypeError("TypeVar cannot have both bound and constraints")
    return _TypeVar(
        name=name,
        covariant=covariant,
        contravariant=contravariant,
        bound=bound,
        constraints=tuple(constraints),
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


class Generic:
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "Generic")


class _ProtocolMeta(type):
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
        attrs = set(getattr(cls, "__annotations__", {}).keys())
        ignored = {
            "__dict__",
            "__weakref__",
            "__module__",
            "__doc__",
            "__annotations__",
            "_is_protocol",
            "_is_runtime_protocol",
            "__protocol_attrs__",
        }
        for key in cls.__dict__:
            if key in ignored:
                continue
            attrs.add(key)
        cls.__protocol_attrs__ = frozenset(attrs)
        cls._is_runtime_protocol = False

    def __instancecheck__(cls, instance) -> bool:
        if not getattr(cls, "_is_runtime_protocol", False):
            raise TypeError(
                "Instance and class checks can only be used with @runtime_checkable protocols"
            )
        return _structural_check(cls, instance)

    def __subclasscheck__(cls, subclass) -> bool:
        if not getattr(cls, "_is_runtime_protocol", False):
            raise TypeError(
                "Instance and class checks can only be used with @runtime_checkable protocols"
            )
        return _structural_check(cls, subclass)


def _structural_check(proto, obj) -> bool:
    for name in getattr(proto, "__protocol_attrs__", ()):
        if not hasattr(obj, name):
            return False
    return True


class Protocol(metaclass=_ProtocolMeta):
    @classmethod
    def __class_getitem__(cls, params: object) -> _GenericAlias:
        return _GenericAlias(cls, params, "Protocol")


_PROTOCOL_BASE: type | None = None


def runtime_checkable(cls):
    if not getattr(cls, "_is_protocol", False):
        raise TypeError("@runtime_checkable can only be applied to protocol classes")
    cls._is_runtime_protocol = True
    return cls


@runtime_checkable
class SupportsIndex(Protocol):
    def __index__(self) -> int:
        raise NotImplementedError


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
    return value


def overload(func):
    return func


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
        return getattr(_abc, "Callable")
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


def _eval_type(value: object, globalns: dict, localns: dict) -> object:
    if isinstance(value, ForwardRef):
        return eval(value.__forward_arg__, globalns, localns)
    if isinstance(value, str):
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
