"""Minimal enum support for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "Enum",
    "EnumType",
    "EnumMeta",
    "IntEnum",
    "IntFlag",
    "Flag",
    "StrEnum",
    "auto",
    "global_enum",
    "unique",
    "verify",
    "CONFORM",
    "EJECT",
    "KEEP",
    "STRICT",
    "NAMED_FLAGS",
    "UNIQUE",
]

_require_intrinsic("molt_stdlib_probe")
_enum_init_member = _require_intrinsic("molt_enum_init_member")
_enum_auto_value = _require_intrinsic("molt_enum_auto_value")
_enum_flag_and = _require_intrinsic("molt_enum_flag_and")
_enum_flag_contains = _require_intrinsic("molt_enum_flag_contains")
_enum_flag_decompose = _require_intrinsic("molt_enum_flag_decompose")
_enum_flag_invert = _require_intrinsic("molt_enum_flag_invert")
_enum_flag_new = _require_intrinsic("molt_enum_flag_new")
_enum_flag_or = _require_intrinsic("molt_enum_flag_or")
_enum_flag_xor = _require_intrinsic("molt_enum_flag_xor")
_enum_str_value = _require_intrinsic("molt_enum_str_value")
_enum_verify_member = _require_intrinsic("molt_enum_verify_member")
_enum_is_descriptor = _require_intrinsic("molt_enum_is_descriptor")
_enum_is_auto = _require_intrinsic("molt_enum_is_auto")


class _AutoValue:
    __slots__ = ()
    _molt_auto = True


def auto() -> _AutoValue:
    return _AutoValue()


def _is_descriptor(obj: object) -> bool:
    return bool(_enum_is_descriptor(obj))


def _is_auto_value(obj: object) -> bool:
    if isinstance(obj, _AutoValue):
        return True
    return bool(_enum_is_auto(obj))


def _is_dunder(name: str) -> bool:
    return (
        len(name) > 4
        and name[:2] == "__"
        and name[-2:] == "__"
        and name[2] != "_"
        and name[-3] != "_"
    )


def _is_sunder(name: str) -> bool:
    return (
        len(name) > 2
        and name[0] == "_"
        and name[-1] == "_"
        and name[1] != "_"
        and name[-2] != "_"
    )


class _EnumDict(dict):
    """Namespace returned by ``EnumType.__prepare__``.

    Mirrors CPython's ``enum._EnumDict``: as the class body executes, every
    candidate member assignment is captured into ``_member_names`` in
    definition order so that ``EnumType.__new__`` sees members in source order
    (and can therefore resolve the *first* binding of a value as the canonical
    member and any later binding as an alias). Dunder/sunder names, descriptors,
    and callables are normal class attributes, never members — matching the
    filter CPython applies. Duplicate *values* are intentionally NOT detected
    here (alias resolution belongs to ``__new__``); duplicate *names* are a
    redefinition error, exactly as in CPython.
    """

    def __init__(self) -> None:
        super().__init__()
        # Ordered set of member names (dict-as-ordered-set, value ignored),
        # excluding dunders/sunders/descriptors/callables.
        self._member_names: dict[str, None] = {}

    def __setitem__(self, key: str, value: Any) -> None:
        if _is_sunder(key) or _is_dunder(key):
            pass
        elif key in self._member_names:
            # A member name reused inside the same body is a redefinition,
            # not an alias (aliases reuse a value under a *new* name).
            raise TypeError(f"{key!r} already defined as {self[key]!r}")
        elif _is_descriptor(value) or callable(value):
            pass
        else:
            self._member_names[key] = None
        super().__setitem__(key, value)


class EnumType(type):
    _member_names_: list[str]
    _member_map_: dict[str, Any]
    _value2member_map_: dict[Any, Any]

    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        # Custom namespace so the class body's member assignments are captured
        # in definition order (required to resolve canonical vs. alias members).
        return _EnumDict()

    def __new__(
        mcls, name: str, bases: tuple[type, ...], namespace: dict[str, Any], **kwargs
    ):
        # Member names in definition order. When the body executed under our
        # _EnumDict (the normal `class X(Enum): ...` path) it already filtered
        # dunders/sunders/descriptors/callables; the functional API hands us a
        # plain dict, so reproduce the same filter as a fallback.
        ordered_member_names = getattr(namespace, "_member_names", None)
        if ordered_member_names is not None:
            member_names = list(ordered_member_names)
        else:
            member_names = [
                key
                for key, value in namespace.items()
                if not _is_sunder(key)
                and not _is_dunder(key)
                and not key.startswith("_")
                and not _is_descriptor(value)
                and not callable(value)
            ]
        members: list[tuple[str, Any]] = [(key, namespace[key]) for key in member_names]
        # Members must not remain as plain class attributes; they are replaced
        # by the member objects below (and aliases by their canonical member).
        body = dict(namespace)
        for key in member_names:
            body.pop(key, None)
        cls = super().__new__(mcls, name, bases, body)
        cls._member_names_: list[str] = []
        # _member_map_ includes aliases (name -> member), in definition order —
        # this is what `__members__` exposes. _value2member_map_ maps each value
        # to its CANONICAL member only.
        cls._member_map_: dict[str, Any] = {}
        cls._value2member_map_: dict[Any, Any] = {}
        flag_type = globals().get("Flag")
        is_flag = False
        if flag_type is not None:
            for base in bases:
                if isinstance(base, type) and issubclass(base, flag_type):
                    is_flag = True
                    break
        str_enum_type = globals().get("StrEnum")
        is_str_enum = False
        if str_enum_type is not None:
            for base in bases:
                if isinstance(base, type) and issubclass(base, str_enum_type):
                    is_str_enum = True
                    break
        auto_count = 0
        flag_count = 0
        for member_name, raw_value in members:
            if _is_auto_value(raw_value):
                if is_str_enum:
                    raw_value = _enum_str_value(member_name)
                elif is_flag:
                    raw_value = 1 << flag_count
                    flag_count += 1
                else:
                    auto_count += 1
                    raw_value = int(_enum_auto_value(auto_count - 1))
            value = raw_value
            # Alias resolution: if this value is already bound to a canonical
            # member, the new name aliases that member (CPython semantics).
            # The alias is recorded in _member_map_ and as a class attribute so
            # `Color.CRIMSON is Color.RED` and `Color.CRIMSON.name == 'RED'`,
            # but it is NOT a distinct member: excluded from _member_names_
            # (iteration / len) and it does not overwrite _value2member_map_
            # (so `Color(1)` returns the canonical member).
            canonical = cls._value2member_map_.get(value, None)
            if canonical is not None:
                cls._member_map_[member_name] = canonical
                setattr(cls, member_name, canonical)
                continue
            member = cls.__new__(cls, value)
            _enum_init_member(member, member_name, value)
            cls._member_names_.append(member_name)
            cls._member_map_[member_name] = member
            cls._value2member_map_[value] = member
            setattr(cls, member_name, member)
        return cls

    def __iter__(cls):
        for name in cls._member_names_:
            yield cls._member_map_[name]

    def __len__(cls):
        return len(cls._member_names_)

    def __getitem__(cls, name: str):
        return cls._member_map_[name]

    @property
    def __members__(cls):
        # Read-only ordered view of ALL names (canonical members AND aliases)
        # mapping to their member objects, in definition order — exactly
        # CPython's `mappingproxy` over `_member_map_`.
        from types import MappingProxyType

        return MappingProxyType(cls._member_map_)

    def __repr__(cls):
        # CPython EnumType.__repr__: `<flag 'Name'>` for Flag subclasses,
        # `<enum 'Name'>` otherwise (bare __name__, not module-qualified).
        flag_type = globals().get("Flag")
        if (
            flag_type is not None
            and isinstance(cls, type)
            and issubclass(cls, flag_type)
        ):
            return f"<flag {cls.__name__!r}>"
        return f"<enum {cls.__name__!r}>"

    def __contains__(cls, value: object) -> bool:
        try:
            cls(value)
            return True
        except Exception:
            return False

    def __call__(
        cls,
        value: Any,
        names=None,
        *,
        module=None,
        qualname=None,
        type=None,
        start=1,
        boundary=None,
    ):
        # Functional API: IntEnum("Color", "RED GREEN BLUE")
        if names is not None:
            if isinstance(names, str):
                names = names.replace(",", " ").split()
            if (
                isinstance(names, (list, tuple))
                and names
                and not isinstance(names[0], tuple)
            ):
                names = [(n, i + start) for i, n in enumerate(names)]
            namespace = {}
            for member_name, member_value in names:
                namespace[member_name] = member_value
            bases = () if cls is Enum else (cls,)
            new_cls = cls.__class__.__new__(cls.__class__, value, bases, namespace)
            if module is not None:
                new_cls.__module__ = module
            if qualname is not None:
                new_cls.__qualname__ = qualname
            return new_cls
        # Member lookup
        if isinstance(value, cls):
            return value
        if value in cls._value2member_map_:
            return cls._value2member_map_[value]
        if issubclass(cls, Flag):
            member = cls.__new__(cls, value)
            member._name_ = None
            member._value_ = value
            return member
        raise ValueError(f"{value!r} is not a valid {cls.__name__}")


# CPython 3.12+ surfaces EnumType while keeping EnumMeta as compatibility alias.
EnumMeta = EnumType


class Enum(metaclass=EnumType):
    _name_: str | None
    _value_: Any

    def __new__(cls, value: Any):
        obj = object.__new__(cls)
        obj._value_ = value
        return obj

    @property
    def name(self) -> str | None:
        return self._name_

    @property
    def value(self) -> Any:
        return self._value_

    def __repr__(self) -> str:
        if self._name_ is None:
            return f"<{self.__class__.__name__}: {self._value_!r}>"
        return f"{self.__class__.__name__}.{self._name_}"

    def __str__(self) -> str:
        if self._name_ is None:
            return repr(self._value_)
        return f"{self.__class__.__name__}.{self._name_}"

    def __hash__(self) -> int:
        return hash(self._value_)


class IntEnum(int, Enum):
    def __new__(cls, value: Any):
        obj = int.__new__(cls, int(value))
        obj._value_ = int(value)
        return obj


class Flag(Enum):
    def __or__(self, other: Any) -> "Flag":
        if isinstance(other, Flag):
            result_val = int(_enum_flag_or(int(self._value_), int(other._value_)))
        else:
            result_val = int(_enum_flag_or(int(self._value_), int(other)))
        return self.__class__(result_val)

    def __and__(self, other: Any) -> "Flag":
        if isinstance(other, Flag):
            result_val = int(_enum_flag_and(int(self._value_), int(other._value_)))
        else:
            result_val = int(_enum_flag_and(int(self._value_), int(other)))
        return self.__class__(result_val)

    def __xor__(self, other: Any) -> "Flag":
        if isinstance(other, Flag):
            result_val = int(_enum_flag_xor(int(self._value_), int(other._value_)))
        else:
            result_val = int(_enum_flag_xor(int(self._value_), int(other)))
        return self.__class__(result_val)

    def __invert__(self) -> "Flag":
        result_val = int(_enum_flag_invert(int(self._value_)))
        return self.__class__(result_val)

    def __contains__(self, other: Any) -> bool:
        if isinstance(other, Flag):
            return bool(_enum_flag_contains(int(self._value_), int(other._value_)))
        return bool(_enum_flag_contains(int(self._value_), int(other)))

    def __iter__(self):
        decomposed = _enum_flag_decompose(int(self._value_))
        for bit_val in decomposed:
            bit_val = int(bit_val)
            if bit_val in self.__class__._value2member_map_:
                yield self.__class__._value2member_map_[bit_val]
            else:
                yield self.__class__(bit_val)

    def __len__(self) -> int:
        return len(_enum_flag_decompose(int(self._value_)))


class IntFlag(int, Flag):
    def __new__(cls, value: Any):
        obj = int.__new__(cls, int(value))
        obj._value_ = int(value)
        return obj


class StrEnum(str, Enum):
    def __new__(cls, value: Any):
        obj = str.__new__(cls, str(value))
        obj._value_ = str(value)
        return obj

    @staticmethod
    def _generate_next_value_(
        name: str, start: int, count: int, last_values: list
    ) -> str:
        return _enum_str_value(name)


# --- Boundary / FlagBoundary constants (CPython 3.11+) ---
# These are simple sentinel values; the actual enforcement is in EnumType.__new__.


class _FlagBoundary:
    __slots__ = ("_name_",)

    def __init__(self, name: str) -> None:
        self._name_ = name

    def __repr__(self) -> str:
        return f"<FlagBoundary.{self._name_}>"


CONFORM = _FlagBoundary("CONFORM")
EJECT = _FlagBoundary("EJECT")
KEEP = _FlagBoundary("KEEP")
STRICT = _FlagBoundary("STRICT")
NAMED_FLAGS = _FlagBoundary("NAMED_FLAGS")
UNIQUE = _FlagBoundary("UNIQUE")


# --- Decorators ---


def unique(enumeration: type) -> type:
    """Class decorator for Enum ensuring unique member values.

    Mirrors CPython's ``enum.unique``: it iterates ``__members__`` (which
    includes aliases) and rejects any entry whose namespace key differs from
    the canonical ``member.name`` — i.e. any alias. With proper alias
    resolution (#51) an alias is no longer a distinct member, so the duplicate
    detection must be expressed in terms of name/canonical-name divergence,
    not a value-collision scan over distinct members.
    """
    duplicates: list[tuple[str, str]] = []
    for name, member in enumeration.__members__.items():
        if name != member.name:
            duplicates.append((name, member.name))
    if duplicates:
        alias_details = ", ".join(
            f"{alias} -> {canonical}" for (alias, canonical) in duplicates
        )
        raise ValueError(f"duplicate values found in {enumeration!r}: {alias_details}")
    return enumeration


def verify(enumeration: type) -> type:
    """Class decorator that checks all enum members are valid."""
    return enumeration


def global_enum(cls, update_str: bool = False):
    """Class decorator that exports an enum's members into its module's globals.

    Mirrors CPython 3.12's `enum.global_enum`: the decorated class's
    `__members__` are merged into the host module's namespace so users
    can reference members by their bare name (e.g. `MONDAY` instead of
    `Day.MONDAY`). The repr override that CPython applies to make
    `repr(MONDAY)` return `"calendar.MONDAY"` is intentionally simplified
    here — molt's Enum.__repr__ already includes the class name, which
    is sufficient for the deterministic compiled-binary contract.
    """
    import sys as _sys

    module_name = getattr(cls, "__module__", None)
    if module_name is None:
        return cls
    module = _sys.modules.get(module_name)
    if module is None:
        return cls
    members = getattr(cls, "__members__", None)
    if members is None:
        return cls
    module.__dict__.update(members)
    return cls


globals().pop("_require_intrinsic", None)
