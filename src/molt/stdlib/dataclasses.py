"""Minimal dataclasses surface for Molt."""

from __future__ import annotations


class _MissingType:
    def __repr__(self) -> str:
        return "MISSING"


MISSING = _MissingType()


class FrozenInstanceError(AttributeError):
    pass


class Field:
    __slots__ = ("name", "default", "default_factory")

    def __init__(self, default=MISSING, default_factory=MISSING) -> None:
        if default is not MISSING and default_factory is not MISSING:
            raise ValueError("cannot specify both default and default_factory")
        self.name = ""
        self.default = default
        self.default_factory = default_factory

    def __repr__(self) -> str:
        return (
            "Field("
            "name=" + repr(self.name) + ", "
            "default=" + repr(self.default) + ", "
            "default_factory=" + repr(self.default_factory) + ")"
        )


class _DataclassParams:
    __slots__ = ("frozen", "eq", "repr", "slots")

    def __init__(self, frozen: bool, eq: bool, repr: bool, slots: bool) -> None:
        self.frozen = frozen
        self.eq = eq
        self.repr = repr
        self.slots = slots

    def __repr__(self) -> str:
        return (
            "_DataclassParams("
            "frozen=" + repr(self.frozen) + ", "
            "eq=" + repr(self.eq) + ", "
            "repr=" + repr(self.repr) + ", "
            "slots=" + repr(self.slots) + ")"
        )


_PENDING_PARAMS: tuple[bool, bool, bool, bool] | None = None


def _dataclass_init(self, *args, **kwargs):
    fields = self.__class__.__dataclass_fields__
    field_names = list(fields.keys())
    if len(args) > len(field_names):
        raise TypeError("too many positional arguments")
    values: dict[str, object] = {}
    for idx, name in enumerate(field_names):
        if idx < len(args):
            if name in kwargs:
                raise TypeError(f"multiple values for argument '{name}'")
            values[name] = args[idx]
            continue
        if name in kwargs:
            values[name] = kwargs.pop(name)
            continue
        field_obj = fields[name]
        if field_obj.default is not MISSING:
            values[name] = field_obj.default
        elif field_obj.default_factory is not MISSING:
            values[name] = field_obj.default_factory()
        else:
            raise TypeError(f"missing required argument: '{name}'")
    if kwargs:
        unexpected = next(iter(kwargs))
        raise TypeError(f"unexpected keyword argument '{unexpected}'")
    params = getattr(self.__class__, "__dataclass_params__", None)
    frozen = getattr(params, "frozen", False)
    for name in field_names:
        if frozen:
            object.__setattr__(self, name, values[name])
        else:
            setattr(self, name, values[name])


def _dataclass_repr(self) -> str:
    cls = self.__class__
    fields = cls.__dataclass_fields__
    parts = []
    for name in fields.keys():
        parts.append(f"{name}={getattr(self, name)!r}")
    return f"{cls.__name__}({', '.join(parts)})"


def _dataclass_eq(self, other: object):
    if other.__class__ is self.__class__:
        fields = self.__class__.__dataclass_fields__
        return all(
            getattr(self, name) == getattr(other, name) for name in fields.keys()
        )
    return NotImplemented


def _dataclass_frozen_setattr(self, name: str, value: object) -> None:
    raise FrozenInstanceError(f"cannot assign to field '{name}'")


def _dataclass_frozen_delattr(self, name: str) -> None:
    raise FrozenInstanceError(f"cannot delete field '{name}'")


def _apply_dataclass(cls, frozen: bool, eq: bool, repr: bool, slots: bool):
    annotations = getattr(cls, "__annotations__", {})
    fields: dict[str, Field] = {}
    for name in annotations:
        default = getattr(cls, name, MISSING)
        if isinstance(default, Field):
            field_obj = default
        else:
            field_obj = Field(default, MISSING)
        if not field_obj.name:
            field_obj.name = name
        fields[name] = field_obj
    cls.__dataclass_fields__ = fields
    cls.__dataclass_params__ = _DataclassParams(frozen, eq, repr, slots)
    if "__init__" not in cls.__dict__:
        cls.__init__ = _dataclass_init
    if repr and "__repr__" not in cls.__dict__:
        cls.__repr__ = _dataclass_repr
    if eq and "__eq__" not in cls.__dict__:
        cls.__eq__ = _dataclass_eq
    if frozen:
        cls.__setattr__ = _dataclass_frozen_setattr
        cls.__delattr__ = _dataclass_frozen_delattr
    if slots:
        cls.__slots__ = tuple(fields.keys())
    return cls


def _dataclass_wrap(cls):
    global _PENDING_PARAMS
    params = _PENDING_PARAMS
    _PENDING_PARAMS = None
    if params is None:
        return cls
    frozen = params[0]
    eq = params[1]
    repr = params[2]
    slots = params[3]
    return _apply_dataclass(cls, frozen, eq, repr, slots)


def dataclass(
    _cls=None,
    frozen: bool = False,
    eq: bool = True,
    repr: bool = True,
    slots: bool = False,
):
    if _cls is None:
        global _PENDING_PARAMS
        _PENDING_PARAMS = (frozen, eq, repr, slots)
        return _dataclass_wrap
    return _apply_dataclass(_cls, frozen, eq, repr, slots)


def field(default=MISSING, default_factory=MISSING):
    return Field(default, default_factory)


__all__ = ["Field", "FrozenInstanceError", "MISSING", "dataclass", "field"]
