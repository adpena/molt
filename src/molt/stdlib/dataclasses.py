"""Dataclasses for Molt (static-only)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())

import copy
from reprlib import recursive_repr
from types import MappingProxyType
from typing import ClassVar


class _MISSING_TYPE:
    def __repr__(self) -> str:
        return "MISSING"


MISSING = _MISSING_TYPE()


class _KW_ONLY_TYPE:
    def __repr__(self) -> str:
        return "KW_ONLY"


KW_ONLY = _KW_ONLY_TYPE()


class FrozenInstanceError(AttributeError):
    pass


class _FieldType:
    __slots__ = ("name",)

    def __init__(self, name: str) -> None:
        self.name = name

    def __repr__(self) -> str:
        return self.name


_FIELD = _FieldType("_FIELD")
_FIELD_CLASSVAR = _FieldType("_FIELD_CLASSVAR")
_FIELD_INITVAR = _FieldType("_FIELD_INITVAR")


class Field:
    __slots__ = (
        "name",
        "type",
        "default",
        "default_factory",
        "repr",
        "hash",
        "init",
        "compare",
        "metadata",
        "kw_only",
        "doc",
        "_field_type",
    )

    def __init__(
        self,
        default=MISSING,
        default_factory=MISSING,
        *,
        init: bool = True,
        repr: bool = True,
        hash=None,
        compare: bool = True,
        metadata=None,
        kw_only=MISSING,
        doc=None,
    ) -> None:
        if default is not MISSING and default_factory is not MISSING:
            raise ValueError("cannot specify both default and default_factory")
        if metadata is None:
            metadata = {}
        else:
            metadata = dict(metadata)
        self.name = None
        self.type = None
        self.default = default
        self.default_factory = default_factory
        self.repr = repr
        self.hash = hash
        self.init = init
        self.compare = compare
        self.metadata = MappingProxyType(metadata)
        self.kw_only = kw_only
        self.doc = doc
        self._field_type = _FIELD

    def __repr__(self) -> str:
        return (
            "Field("
            f"name={self.name!r},"
            f"type={self.type!r},"
            f"default={self.default!r},"
            f"default_factory={self.default_factory!r},"
            f"init={self.init!r},"
            f"repr={self.repr!r},"
            f"hash={self.hash!r},"
            f"compare={self.compare!r},"
            f"metadata={self.metadata!r},"
            f"kw_only={self.kw_only!r},"
            f"doc={self.doc!r},"
            f"_field_type={self._field_type!r})"
        )


class InitVar:
    __slots__ = ("type",)

    def __init__(self, type) -> None:
        self.type = type

    def __repr__(self) -> str:
        return f"dataclasses.InitVar[{self.type!r}]"

    def __class_getitem__(cls, item):
        return cls(item)


class _DataclassParams:
    __slots__ = (
        "init",
        "repr",
        "eq",
        "order",
        "unsafe_hash",
        "frozen",
        "match_args",
        "kw_only",
        "slots",
        "weakref_slot",
    )

    def __init__(
        self,
        init: bool,
        repr: bool,
        eq: bool,
        order: bool,
        unsafe_hash: bool,
        frozen: bool,
        match_args: bool,
        kw_only: bool,
        slots: bool,
        weakref_slot: bool,
    ) -> None:
        self.init = init
        self.repr = repr
        self.eq = eq
        self.order = order
        self.unsafe_hash = unsafe_hash
        self.frozen = frozen
        self.match_args = match_args
        self.kw_only = kw_only
        self.slots = slots
        self.weakref_slot = weakref_slot

    def __repr__(self) -> str:
        return (
            "_DataclassParams("
            f"init={self.init!r}, "
            f"repr={self.repr!r}, "
            f"eq={self.eq!r}, "
            f"order={self.order!r}, "
            f"unsafe_hash={self.unsafe_hash!r}, "
            f"frozen={self.frozen!r}, "
            f"match_args={self.match_args!r}, "
            f"kw_only={self.kw_only!r}, "
            f"slots={self.slots!r}, "
            f"weakref_slot={self.weakref_slot!r})"
        )


def field(
    *,
    default=MISSING,
    default_factory=MISSING,
    init: bool = True,
    repr: bool = True,
    hash=None,
    compare: bool = True,
    metadata=None,
    kw_only=MISSING,
    doc=None,
):
    return Field(
        default,
        default_factory,
        init=init,
        repr=repr,
        hash=hash,
        compare=compare,
        metadata=metadata,
        kw_only=kw_only,
        doc=doc,
    )


def _is_classvar(annotation) -> bool:
    try:
        if annotation is ClassVar:
            return True
        origin = getattr(annotation, "__origin__", None)
        return origin is ClassVar
    except Exception:
        return False


def _is_initvar(annotation) -> bool:
    if annotation is InitVar:
        return True
    return isinstance(annotation, InitVar)


def _is_kw_only(annotation) -> bool:
    return annotation is KW_ONLY or isinstance(annotation, _KW_ONLY_TYPE)


def _has_default(field_obj: Field) -> bool:
    return field_obj.default is not MISSING or field_obj.default_factory is not MISSING


def _check_default_order(fields: dict[str, Field]) -> None:
    seen_default = None
    for field_obj in fields.values():
        if field_obj._field_type not in (_FIELD, _FIELD_INITVAR):
            continue
        if not field_obj.init or field_obj.kw_only:
            continue
        if _has_default(field_obj):
            seen_default = field_obj
            continue
        if seen_default is not None:
            raise TypeError(
                "non-default argument "
                f"{field_obj.name!r} follows default argument "
                f"{seen_default.name!r}"
            )


def _dataclass_field_flags(fields: dict[str, Field]) -> tuple[int, ...]:
    flags: list[int] = []
    for field_obj in fields.values():
        if field_obj._field_type is not _FIELD:
            continue
        flag = 0
        if field_obj.repr:
            flag |= 0x1
        if field_obj.compare:
            flag |= 0x2
        hash_flag = field_obj.hash
        if hash_flag is None:
            hash_flag = field_obj.compare
        if hash_flag:
            flag |= 0x4
        flags.append(flag)
    return tuple(flags)


def _should_set_hash(
    *,
    has_explicit_hash: bool,
    eq: bool,
    frozen: bool,
    unsafe_hash: bool,
) -> str:
    if unsafe_hash:
        return "set"
    if has_explicit_hash:
        return "leave"
    if not eq:
        return "leave"
    if frozen:
        return "set"
    return "none"


def _dataclass_hash_mode(
    *,
    has_explicit_hash: bool,
    hash_is_none: bool,
    eq: bool,
    frozen: bool,
    unsafe_hash: bool,
) -> int:
    if unsafe_hash:
        return 1
    if hash_is_none:
        return 2
    if has_explicit_hash:
        return 3
    if not eq:
        return 0
    if frozen:
        return 1
    return 2


def _dataclass_init(self, *args, **kwargs):
    cls = self.__class__
    fields_map = getattr(cls, "__dataclass_fields__", None)
    if fields_map is None:
        raise TypeError("dataclass __init__ called on non-dataclass instance")
    params = getattr(cls, "__dataclass_params__", None)
    frozen = getattr(params, "frozen", False)
    positional: list[Field] = []
    kw_only: list[Field] = []
    initvars: list[Field] = []
    for field_obj in fields_map.values():
        if field_obj._field_type not in (_FIELD, _FIELD_INITVAR):
            continue
        if not field_obj.init:
            continue
        if field_obj.kw_only:
            kw_only.append(field_obj)
        else:
            positional.append(field_obj)
        if field_obj._field_type is _FIELD_INITVAR:
            initvars.append(field_obj)
    init_name = f"{cls.__name__}.__init__"
    if len(args) > len(positional):
        total = len(positional) + 1
        given = len(args) + 1
        raise TypeError(
            f"{init_name}() takes {total} positional arguments but {given} were given"
        )
    values: dict[str, object] = {}
    for idx, field_obj in enumerate(positional):
        if idx < len(args):
            if field_obj.name in kwargs:
                raise TypeError(
                    f"{init_name}() got multiple values for argument '{field_obj.name}'"
                )
            values[field_obj.name] = args[idx]
            continue
        if field_obj.name in kwargs:
            values[field_obj.name] = kwargs.pop(field_obj.name)
            continue
        if field_obj.default is not MISSING:
            values[field_obj.name] = field_obj.default
        elif field_obj.default_factory is not MISSING:
            values[field_obj.name] = field_obj.default_factory()
        else:
            raise TypeError(
                f"{init_name}() missing 1 required positional argument: '{field_obj.name}'"
            )
    for field_obj in kw_only:
        if field_obj.name in kwargs:
            values[field_obj.name] = kwargs.pop(field_obj.name)
            continue
        if field_obj.default is not MISSING:
            values[field_obj.name] = field_obj.default
        elif field_obj.default_factory is not MISSING:
            values[field_obj.name] = field_obj.default_factory()
        else:
            raise TypeError(
                f"{init_name}() missing 1 required keyword-only argument: '{field_obj.name}'"
            )
    if kwargs:
        unexpected = next(iter(kwargs))
        raise TypeError(
            f"{init_name}() got an unexpected keyword argument '{unexpected}'"
        )
    initvar_values: list[object] = []
    for field_obj in fields_map.values():
        if field_obj._field_type is _FIELD:
            if field_obj.init:
                val = values[field_obj.name]
            else:
                if field_obj.default is not MISSING:
                    val = field_obj.default
                elif field_obj.default_factory is not MISSING:
                    val = field_obj.default_factory()
                else:
                    continue
            if frozen:
                object.__setattr__(self, field_obj.name, val)
            else:
                setattr(self, field_obj.name, val)
        elif field_obj._field_type is _FIELD_INITVAR and field_obj.init:
            initvar_values.append(values[field_obj.name])
    post_init = getattr(self, "__post_init__", None)
    if post_init is not None:
        post_init(*initvar_values)


@recursive_repr()
def _dataclass_repr(self) -> str:
    cls = self.__class__
    fields_map = getattr(cls, "__dataclass_fields__", None)
    if not fields_map:
        return "<dataclass>"
    parts = []
    for field_obj in fields_map.values():
        if field_obj._field_type is not _FIELD or not field_obj.repr:
            continue
        parts.append(f"{field_obj.name}={getattr(self, field_obj.name)!r}")
    name = getattr(cls, "__qualname__", cls.__name__)
    return f"{name}({', '.join(parts)})"


def _dataclass_eq(self, other: object):
    if other.__class__ is self.__class__:
        fields_map = self.__class__.__dataclass_fields__
        return all(
            getattr(self, field_obj.name) == getattr(other, field_obj.name)
            for field_obj in fields_map.values()
            if field_obj._field_type is _FIELD and field_obj.compare
        )
    return NotImplemented


def _dataclass_order(self, other: object, op):
    if other.__class__ is not self.__class__:
        return NotImplemented
    fields_map = self.__class__.__dataclass_fields__
    values = [
        getattr(self, field_obj.name)
        for field_obj in fields_map.values()
        if field_obj._field_type is _FIELD and field_obj.compare
    ]
    other_values = [
        getattr(other, field_obj.name)
        for field_obj in fields_map.values()
        if field_obj._field_type is _FIELD and field_obj.compare
    ]
    return op(tuple(values), tuple(other_values))


def _dataclass_hash(self) -> int:
    fields_map = self.__class__.__dataclass_fields__
    values = []
    for field_obj in fields_map.values():
        if field_obj._field_type is not _FIELD:
            continue
        hash_flag = field_obj.hash
        if hash_flag is None:
            hash_flag = field_obj.compare
        if hash_flag:
            values.append(getattr(self, field_obj.name))
    return hash(tuple(values))


def _molt_apply_dataclass(
    cls,
    init: bool,
    repr: bool,
    eq: bool,
    order: bool,
    unsafe_hash: bool,
    frozen: bool,
    match_args: bool,
    kw_only: bool,
    slots: bool,
    weakref_slot: bool,
):
    if not getattr(cls, "__molt_dataclass__", False):
        raise NotImplementedError(
            "dataclasses.dataclass is static-only in Molt (use @dataclass at compile time)"
        )
    if order and not eq:
        raise ValueError("eq must be true if order is true")
    if weakref_slot and not slots:
        raise TypeError("weakref_slot is True but slots is False")
    if slots and "__slots__" in cls.__dict__:
        raise TypeError(f"{cls.__name__} already specifies __slots__")
    if order:
        for name in ("__lt__", "__le__", "__gt__", "__ge__"):
            if name in cls.__dict__:
                raise TypeError(
                    f"Cannot overwrite attribute {name} in class {cls.__name__}"
                )
    if frozen:
        for name in ("__setattr__", "__delattr__"):
            if name in cls.__dict__:
                raise TypeError(
                    f"Cannot overwrite attribute {name} in class {cls.__name__}"
                )
    if unsafe_hash and "__hash__" in cls.__dict__:
        raise TypeError(f"Cannot overwrite attribute __hash__ in class {cls.__name__}")

    fields: dict[str, Field] = {}
    for base in cls.__mro__[1:]:
        base_fields = getattr(base, "__dataclass_fields__", None)
        if base_fields:
            for name, field_obj in base_fields.items():
                fields[name] = field_obj

    annotations = getattr(cls, "__annotations__", {}) or {}
    kw_only_marker = kw_only
    for name, annotation in annotations.items():
        if _is_kw_only(annotation):
            kw_only_marker = True
            continue
        default = cls.__dict__.get(name, MISSING)
        if isinstance(default, Field):
            field_obj = default
        else:
            field_obj = Field(default)
        field_obj.name = name
        field_obj.type = annotation
        if _is_classvar(annotation):
            field_obj._field_type = _FIELD_CLASSVAR
        elif _is_initvar(annotation):
            field_obj._field_type = _FIELD_INITVAR
        else:
            field_obj._field_type = _FIELD
        if field_obj.kw_only is MISSING:
            field_obj.kw_only = kw_only_marker
        fields[name] = field_obj

    _check_default_order(fields)

    for field_obj in fields.values():
        if field_obj.default_factory is not MISSING:
            if isinstance(cls.__dict__.get(field_obj.name), Field):
                delattr(cls, field_obj.name)
            continue
        if field_obj.default is MISSING:
            if isinstance(cls.__dict__.get(field_obj.name), Field):
                delattr(cls, field_obj.name)
            continue
        setattr(cls, field_obj.name, field_obj.default)

    params = _DataclassParams(
        init=init,
        repr=repr,
        eq=eq,
        order=order,
        unsafe_hash=unsafe_hash,
        frozen=frozen,
        match_args=match_args,
        kw_only=kw_only,
        slots=slots,
        weakref_slot=weakref_slot,
    )
    cls.__dataclass_fields__ = fields
    cls.__dataclass_params__ = params

    if init and "__init__" not in cls.__dict__:
        cls.__init__ = _dataclass_init
    if repr and "__repr__" not in cls.__dict__:
        cls.__repr__ = _dataclass_repr
    if eq and "__eq__" not in cls.__dict__:
        cls.__eq__ = _dataclass_eq
    if order:
        if "__lt__" not in cls.__dict__:
            cls.__lt__ = lambda self, other: _dataclass_order(
                self, other, lambda a, b: a < b
            )
        if "__le__" not in cls.__dict__:
            cls.__le__ = lambda self, other: _dataclass_order(
                self, other, lambda a, b: a <= b
            )
        if "__gt__" not in cls.__dict__:
            cls.__gt__ = lambda self, other: _dataclass_order(
                self, other, lambda a, b: a > b
            )
        if "__ge__" not in cls.__dict__:
            cls.__ge__ = lambda self, other: _dataclass_order(
                self, other, lambda a, b: a >= b
            )

    hash_obj = cls.__dict__.get("__hash__", MISSING)
    has_explicit_hash = "__hash__" in cls.__dict__
    hash_is_none = has_explicit_hash and hash_obj is None
    hash_action = _should_set_hash(
        has_explicit_hash=has_explicit_hash,
        eq=eq,
        frozen=frozen,
        unsafe_hash=unsafe_hash,
    )
    if hash_action == "set":
        cls.__hash__ = _dataclass_hash
    elif hash_action == "none":
        cls.__hash__ = None

    if frozen:
        cls.__setattr__ = _dataclass_frozen_setattr
        cls.__delattr__ = _dataclass_frozen_delattr

    if match_args:
        match_fields = tuple(
            field_obj.name
            for field_obj in fields.values()
            if field_obj._field_type is _FIELD
            and field_obj.init
            and not field_obj.kw_only
        )
        cls.__match_args__ = match_fields

    if slots:
        slot_names = [
            field_obj.name
            for field_obj in fields.values()
            if field_obj._field_type is _FIELD
        ]
        if weakref_slot:
            slot_names.append("__weakref__")
        cls.__slots__ = tuple(slot_names)

    cls.__molt_dataclass_field_names__ = tuple(
        field_obj.name
        for field_obj in fields.values()
        if field_obj._field_type is _FIELD
    )
    flags = 0
    if frozen:
        flags |= 0x1
    if eq:
        flags |= 0x2
    if repr:
        flags |= 0x4
    if slots:
        flags |= 0x8
    cls.__molt_dataclass_flags__ = flags
    cls.__molt_dataclass_field_flags__ = _dataclass_field_flags(fields)
    cls.__molt_dataclass_hash__ = _dataclass_hash_mode(
        has_explicit_hash=has_explicit_hash,
        hash_is_none=hash_is_none,
        eq=eq,
        frozen=frozen,
        unsafe_hash=unsafe_hash,
    )
    return cls


def _dataclass_frozen_setattr(self, name: str, value: object) -> None:
    raise FrozenInstanceError(f"cannot assign to field '{name}'")


def _dataclass_frozen_delattr(self, name: str) -> None:
    raise FrozenInstanceError(f"cannot delete field '{name}'")


def dataclass(
    _cls=None,
    /,
    *,
    init: bool = True,
    repr: bool = True,
    eq: bool = True,
    order: bool = False,
    unsafe_hash: bool = False,
    frozen: bool = False,
    match_args: bool = True,
    kw_only: bool = False,
    slots: bool = False,
    weakref_slot: bool = False,
):
    if _cls is None:
        return lambda cls: _molt_apply_dataclass(
            cls,
            init=init,
            repr=repr,
            eq=eq,
            order=order,
            unsafe_hash=unsafe_hash,
            frozen=frozen,
            match_args=match_args,
            kw_only=kw_only,
            slots=slots,
            weakref_slot=weakref_slot,
        )
    return _molt_apply_dataclass(
        _cls,
        init=init,
        repr=repr,
        eq=eq,
        order=order,
        unsafe_hash=unsafe_hash,
        frozen=frozen,
        match_args=match_args,
        kw_only=kw_only,
        slots=slots,
        weakref_slot=weakref_slot,
    )


def is_dataclass(obj) -> bool:
    if isinstance(obj, type):
        return hasattr(obj, "__dataclass_fields__")
    return hasattr(obj.__class__, "__dataclass_fields__")


def fields(class_or_instance):
    if not is_dataclass(class_or_instance):
        raise TypeError("must be called with a dataclass type or instance")
    cls = (
        class_or_instance
        if isinstance(class_or_instance, type)
        else class_or_instance.__class__
    )
    return tuple(
        field_obj
        for field_obj in cls.__dataclass_fields__.values()
        if field_obj._field_type is _FIELD
    )


def asdict(obj, *, dict_factory=dict):
    if not is_dataclass(obj) or isinstance(obj, type):
        raise TypeError("asdict() should be called on dataclass instances")

    def _inner(value):
        if is_dataclass(value):
            return dict_factory(
                (field_obj.name, _inner(getattr(value, field_obj.name)))
                for field_obj in fields(value)
            )
        if isinstance(value, (list, tuple)):
            return type(value)(_inner(item) for item in value)
        if isinstance(value, dict):
            return type(value)((_inner(k), _inner(v)) for k, v in value.items())
        return copy.deepcopy(value)

    return _inner(obj)


def astuple(obj, *, tuple_factory=tuple):
    if not is_dataclass(obj) or isinstance(obj, type):
        raise TypeError("astuple() should be called on dataclass instances")

    def _inner(value):
        if is_dataclass(value):
            return tuple_factory(_inner(getattr(value, f.name)) for f in fields(value))
        if isinstance(value, (list, tuple)):
            return type(value)(_inner(item) for item in value)
        if isinstance(value, dict):
            return type(value)((_inner(k), _inner(v)) for k, v in value.items())
        return copy.deepcopy(value)

    return _inner(obj)


def replace(obj, **changes):
    if not is_dataclass(obj) or isinstance(obj, type):
        raise TypeError("replace() should be called on dataclass instances")
    cls = obj.__class__
    values: dict[str, object] = {}
    for field_obj in cls.__dataclass_fields__.values():
        if field_obj._field_type is _FIELD_INITVAR:
            if field_obj.name in changes:
                values[field_obj.name] = changes.pop(field_obj.name)
            else:
                raise TypeError(
                    f"InitVar {field_obj.name!r} must be specified with replace()"
                )
            continue
        if field_obj._field_type is not _FIELD:
            continue
        if not field_obj.init:
            if field_obj.name in changes:
                raise TypeError(
                    f"field {field_obj.name} is declared with init=False, "
                    "it cannot be specified with replace()"
                )
            continue
        if field_obj.name in changes:
            values[field_obj.name] = changes.pop(field_obj.name)
        else:
            values[field_obj.name] = getattr(obj, field_obj.name)
    return cls(**values, **changes)


def make_dataclass(*_args, **_kwargs):
    # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): implement make_dataclass once dynamic class construction is allowed by the runtime contract.
    raise NotImplementedError(
        "make_dataclass is not supported in Molt (static-only dataclasses)"
    )


__all__ = [
    "Field",
    "FrozenInstanceError",
    "InitVar",
    "KW_ONLY",
    "MISSING",
    "asdict",
    "astuple",
    "dataclass",
    "field",
    "fields",
    "is_dataclass",
    "make_dataclass",
    "replace",
]
