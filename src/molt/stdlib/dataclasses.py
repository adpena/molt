"""Minimal dataclasses surface for Molt."""

from __future__ import annotations


class _MissingType:
    def __repr__(self) -> str:
        return "MISSING"


MISSING = _MissingType()


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


__all__ = ["Field", "MISSING", "dataclass", "field"]
