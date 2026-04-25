"""Dataclasses for Molt.

Supports both compile-time recognition (the frontend marks classes with
``__molt_dataclass__ = True``) and runtime decoration of arbitrary user
classes. The synthesis logic below is the single source of truth for both
paths: it walks ``cls.__annotations__`` to discover fields, installs
``__init__`` / ``__repr__`` / ``__eq__`` / ``__hash__`` / order dunders
onto the class via ``setattr``, and honours ``slots`` / ``frozen`` /
``order`` / ``kw_only`` / ``unsafe_hash`` / ``weakref_slot`` / ``match_args``
exactly as CPython 3.12 does.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys
from reprlib import recursive_repr
from types import MappingProxyType
from typing import Any, Callable, ClassVar, cast

_MOLT_DATACLASSES_MAKE_DATACLASS = _require_intrinsic("molt_dataclasses_make_dataclass")
_MOLT_DATACLASSES_IS_DATACLASS = _require_intrinsic("molt_dataclasses_is_dataclass")
_MOLT_DATACLASSES_FIELDS = _require_intrinsic("molt_dataclasses_fields")
_MOLT_DATACLASSES_ASDICT = _require_intrinsic("molt_dataclasses_asdict")
_MOLT_DATACLASSES_ASTUPLE = _require_intrinsic("molt_dataclasses_astuple")
_MOLT_DATACLASSES_REPLACE = _require_intrinsic("molt_dataclasses_replace")
_MOLT_DATACLASSES_REPR = _require_intrinsic("molt_dataclasses_repr")
_MOLT_DATACLASSES_EQ = _require_intrinsic("molt_dataclasses_eq")
_MOLT_DATACLASSES_HASH_FN = _require_intrinsic("molt_dataclasses_hash_fn")
_MOLT_DATACLASSES_CHECK_DEFAULT_ORDER = _require_intrinsic(
    "molt_dataclasses_check_default_order"
)
_MOLT_DATACLASSES_FIELD_FLAGS = _require_intrinsic("molt_dataclasses_field_flags")
_MOLT_DATACLASSES_POST_INIT = _require_intrinsic("molt_dataclasses_post_init")
_MOLT_DATACLASSES_FIELD_METADATA = _require_intrinsic("molt_dataclasses_field_metadata")
_MOLT_DATACLASSES_SET_FIELD_METADATA = _require_intrinsic(
    "molt_dataclasses_set_field_metadata"
)
_MOLT_DATACLASSES_IS_INITVAR = _require_intrinsic("molt_dataclasses_is_initvar")
_MOLT_DATACLASSES_IS_KW_ONLY_SENTINEL = _require_intrinsic(
    "molt_dataclasses_is_kw_only_sentinel"
)


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
        _MOLT_DATACLASSES_SET_FIELD_METADATA(self, self.metadata)

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
    if isinstance(annotation, InitVar):
        return True
    return bool(_MOLT_DATACLASSES_IS_INITVAR(annotation))


def _is_kw_only(annotation) -> bool:
    if annotation is KW_ONLY or isinstance(annotation, _KW_ONLY_TYPE):
        return True
    return bool(_MOLT_DATACLASSES_IS_KW_ONLY_SENTINEL(annotation))


def _check_default_order(fields: dict[str, Field]) -> None:
    _MOLT_DATACLASSES_CHECK_DEFAULT_ORDER(fields)


def _dataclass_field_flags(fields: dict[str, Field]) -> tuple[int, ...]:
    return _MOLT_DATACLASSES_FIELD_FLAGS(fields)


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
        field_name = cast(str, field_obj.name)
        if idx < len(args):
            if field_name in kwargs:
                raise TypeError(
                    f"{init_name}() got multiple values for argument '{field_name}'"
                )
            values[field_name] = args[idx]
            continue
        if field_name in kwargs:
            values[field_name] = kwargs.pop(field_name)
            continue
        if field_obj.default is not MISSING:
            values[field_name] = field_obj.default
        elif field_obj.default_factory is not MISSING:
            factory = cast(Callable[[], object], field_obj.default_factory)
            values[field_name] = factory()
        else:
            raise TypeError(
                f"{init_name}() missing 1 required positional argument: '{field_name}'"
            )
    for field_obj in kw_only:
        field_name = cast(str, field_obj.name)
        if field_name in kwargs:
            values[field_name] = kwargs.pop(field_name)
            continue
        if field_obj.default is not MISSING:
            values[field_name] = field_obj.default
        elif field_obj.default_factory is not MISSING:
            factory = cast(Callable[[], object], field_obj.default_factory)
            values[field_name] = factory()
        else:
            raise TypeError(
                f"{init_name}() missing 1 required keyword-only argument: '{field_name}'"
            )
    if kwargs:
        unexpected = next(iter(kwargs))
        raise TypeError(
            f"{init_name}() got an unexpected keyword argument '{unexpected}'"
        )
    initvar_values: list[object] = []
    for field_obj in fields_map.values():
        if field_obj._field_type is _FIELD:
            field_name = cast(str, field_obj.name)
            if field_obj.init:
                val = values[field_name]
            else:
                if field_obj.default is not MISSING:
                    val = field_obj.default
                elif field_obj.default_factory is not MISSING:
                    factory = cast(Callable[[], object], field_obj.default_factory)
                    val = factory()
                else:
                    continue
            if frozen:
                object.__setattr__(self, field_name, val)
            else:
                setattr(self, field_name, val)
        elif field_obj._field_type is _FIELD_INITVAR and field_obj.init:
            field_name = cast(str, field_obj.name)
            initvar_values.append(values[field_name])
    if getattr(self.__class__, "__molt_dataclass_has_post_init__", False):
        _MOLT_DATACLASSES_POST_INIT(self, initvar_values)


@recursive_repr()
def _dataclass_repr(self) -> str:
    return _MOLT_DATACLASSES_REPR(self)


def _dataclass_eq(self, other: object):
    result = _MOLT_DATACLASSES_EQ(self, other)
    if result is None:
        return NotImplemented
    return result


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
    return int(_MOLT_DATACLASSES_HASH_FN(self))


def _collect_inherited_slots(cls) -> set:
    """Mirror CPython's ``_get_slots`` walk over ``cls.__mro__[1:-1]``.

    Each base contributes its declared ``__slots__`` (string or iterable). We
    skip ``object`` because every class inherits from it and its slots are
    irrelevant. The returned set is used to filter our generated slot names so
    we never re-declare an inherited slot.
    """
    inherited: set = set()
    mro = getattr(cls, "__mro__", None) or (cls,)
    for base in mro[1:-1]:
        base_slots = base.__dict__.get("__slots__", None)
        if base_slots is None:
            continue
        if isinstance(base_slots, str):
            inherited.add(base_slots)
            continue
        try:
            for entry in base_slots:
                if isinstance(entry, str):
                    inherited.add(entry)
        except TypeError:
            # Non-iterable __slots__ on a base — leave to the base class.
            continue
    return inherited


def _add_slots(cls, fields_map, *, frozen: bool, weakref_slot: bool):
    """Rebuild ``cls`` with ``__slots__`` baked into its class dict.

    Mirrors CPython's ``dataclasses._add_slots``: copies the existing class
    namespace (which already contains every dunder synthesized above), drops
    field-name attributes that would shadow slot descriptors, removes any
    existing ``__dict__``/``__weakref__`` entries, then constructs a new class
    via the original metaclass. The original class object is intentionally
    discarded — callers receive the rebuilt class through the decorator return
    value, exactly as CPython does.
    """
    field_names = tuple(
        field_obj.name
        for field_obj in fields_map.values()
        if field_obj._field_type is _FIELD
    )
    inherited_slots = _collect_inherited_slots(cls)
    new_slots = tuple(
        name
        for name in (*field_names, *(("__weakref__",) if weakref_slot else ()))
        if name not in inherited_slots
    )

    # Snapshot the (now fully decorated) class namespace.
    cls_dict = dict(cls.__dict__)
    cls_dict["__slots__"] = new_slots
    for field_name in field_names:
        # Slot descriptors must own the attribute name; strip any default
        # value we wrote earlier in this pass so it does not shadow the slot.
        cls_dict.pop(field_name, None)
    cls_dict.pop("__dict__", None)
    cls_dict.pop("__weakref__", None)

    qualname = getattr(cls, "__qualname__", None)
    bases = cls.__bases__

    # Use ``types.new_class`` (the proven path used by ``make_dataclass``) to
    # rebuild via the original metaclass. This avoids any reliance on the
    # rarely-exercised 3-arg ``type(name, bases, dict)`` path while preserving
    # CPython semantics: a brand-new class object whose namespace contains
    # ``__slots__`` and the synthesized dunders.
    snapshot = cls_dict

    def _exec_body(ns):
        ns.update(snapshot)

    import types as _types

    new_cls = _types.new_class(cls.__name__, bases, {}, _exec_body)
    if qualname is not None:
        new_cls.__qualname__ = qualname

    if frozen:
        # Frozen + slots needs explicit pickle support: __setstate__ must use
        # ``object.__setattr__`` since assignment goes through our frozen
        # ``__setattr__`` guard. Match CPython behaviour exactly.
        if "__getstate__" not in cls_dict:
            new_cls.__getstate__ = _dataclass_slots_getstate
        if "__setstate__" not in cls_dict:
            new_cls.__setstate__ = _dataclass_slots_setstate

    return new_cls


def _dataclass_slots_getstate(self):
    return [getattr(self, name) for name in self.__slots__ if name != "__weakref__"]


def _dataclass_slots_setstate(self, state) -> None:
    names = [name for name in self.__slots__ if name != "__weakref__"]
    for name, value in zip(names, state):
        object.__setattr__(self, name, value)


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
    # Mark the class as a molt dataclass so downstream introspection (and any
    # cooperating frontend lowering) sees the same flag whether the class was
    # produced via compile-time recognition, ``make_dataclass``, or runtime
    # decoration of an arbitrary user class.
    if not getattr(cls, "__molt_dataclass__", False):
        cls.__molt_dataclass__ = True
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
    # PEP 749 (Python 3.14+): annotations may be deferred via __annotate__.
    # If __annotations__ is empty but __annotate__ exists, call it to
    # eagerly evaluate and cache the annotations.
    if not annotations:
        annotate_fn = getattr(cls, "__annotate__", None)
        if annotate_fn is not None:
            try:
                annotations = annotate_fn(1)  # FORMAT_VALUE = 1
                cls.__annotations__ = annotations
            except Exception:
                annotations = {}
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
        field_name = cast(str, field_obj.name)
        if field_obj.default_factory is not MISSING:
            if isinstance(cls.__dict__.get(field_name), Field):
                delattr(cls, field_name)
            continue
        if field_obj.default is MISSING:
            if isinstance(cls.__dict__.get(field_name), Field):
                delattr(cls, field_name)
            continue
        setattr(cls, field_name, field_obj.default)

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
    cls.__molt_dataclass_has_post_init__ = callable(getattr(cls, "__post_init__", None))

    user_init_marker = cls.__dict__.get("__molt_dataclass_user_init__", MISSING)
    from_make_dataclass = bool(cls.__dict__.get("__molt_make_dataclass__", False))
    user_defined_init = False
    if user_init_marker is not MISSING:
        user_defined_init = bool(user_init_marker)
    if init and from_make_dataclass:
        if not user_defined_init:
            cls.__init__ = _dataclass_init
    elif init and (
        "__init__" not in cls.__dict__
        or cls.__dict__.get("__init__") is object.__init__
    ):
        cls.__init__ = _dataclass_init
    if repr and (
        "__repr__" not in cls.__dict__
        or cls.__dict__.get("__repr__") is object.__repr__
    ):
        cls.__repr__ = _dataclass_repr
    if eq and (
        "__eq__" not in cls.__dict__ or cls.__dict__.get("__eq__") is object.__eq__
    ):
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
            if field_obj._field_type in (_FIELD, _FIELD_INITVAR)
            and field_obj.init
            and not field_obj.kw_only
        )
        cls.__match_args__ = match_fields

    if slots:
        cls = _add_slots(cls, fields, frozen=frozen, weakref_slot=weakref_slot)

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
    return bool(_MOLT_DATACLASSES_IS_DATACLASS(obj))


def fields(class_or_instance):
    return _MOLT_DATACLASSES_FIELDS(class_or_instance, _FIELD)


def asdict(obj, *, dict_factory=dict):
    return _MOLT_DATACLASSES_ASDICT(obj, dict_factory, _FIELD)


def astuple(obj, *, tuple_factory=tuple):
    return _MOLT_DATACLASSES_ASTUPLE(obj, tuple_factory, _FIELD)


def replace(obj, **changes):
    return _MOLT_DATACLASSES_REPLACE(obj, changes, _FIELD, _FIELD_INITVAR)


def _infer_caller_module_name() -> str:
    frame = getattr(_sys, "_getframe", None)
    if not callable(frame):
        return "__main__"
    try:
        caller = frame(2)
    except Exception:
        return "__main__"
    globals_obj = getattr(caller, "f_globals", None)
    if isinstance(globals_obj, dict):
        module_name = globals_obj.get("__name__")
        if isinstance(module_name, str) and module_name:
            return module_name
    return "__main__"


def make_dataclass(
    cls_name: str,
    fields,
    *,
    bases: tuple[type, ...] = (),
    namespace: dict[str, object] | None = None,
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
    module: str | None = None,
    decorator=dataclass,
):
    resolved_module = module if module is not None else _infer_caller_module_name()
    prepared = _MOLT_DATACLASSES_MAKE_DATACLASS(
        cls_name,
        fields,
        bases,
        namespace,
        resolved_module,
        Any,
        Field,
    )
    if not isinstance(prepared, tuple) or len(prepared) != 2:
        raise RuntimeError(
            "dataclasses.make_dataclass intrinsic returned invalid state"
        )
    prepared_bases, body = prepared
    if not isinstance(body, dict):
        raise RuntimeError(
            "dataclasses.make_dataclass intrinsic returned invalid namespace"
        )
    if not callable(decorator):
        raise TypeError("decorator must be callable")

    def exec_body(ns):
        ns.update(body)

    import types as _types

    cls = _types.new_class(cls_name, prepared_bases, {}, exec_body)
    if decorator is dataclass:
        # Avoid keyword-binding drift in compiled paths by calling the
        # internal worker positionally for the default dataclass decorator.
        result = _molt_apply_dataclass(
            cls,
            init,
            repr,
            eq,
            order,
            unsafe_hash,
            frozen,
            match_args,
            kw_only,
            slots,
            weakref_slot,
        )
    else:
        result = decorator(
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
    if isinstance(result, type):
        user_defined_init = bool(
            result.__dict__.get("__molt_dataclass_user_init__", False)
        )
        if init and not user_defined_init:
            result.__init__ = _dataclass_init
    return result


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

globals().pop("_require_intrinsic", None)
