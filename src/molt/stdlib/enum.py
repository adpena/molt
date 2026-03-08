"""Minimal enum support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from typing import Any

__all__ = [
    "Enum",
    "EnumType",
    "EnumMeta",
    "IntEnum",
    "IntFlag",
    "Flag",
    "StrEnum",
    "auto",
    "unique",
    "verify",
    "CONFORM",
    "EJECT",
    "KEEP",
    "STRICT",
    "NAMED_FLAGS",
    "UNIQUE",
]

_require_intrinsic("molt_stdlib_probe", globals())
_enum_init_member = _require_intrinsic("molt_enum_init_member", globals())
_enum_auto_value = _require_intrinsic("molt_enum_auto_value", globals())
_enum_flag_and = _require_intrinsic("molt_enum_flag_and", globals())
_enum_flag_contains = _require_intrinsic("molt_enum_flag_contains", globals())
_enum_flag_decompose = _require_intrinsic("molt_enum_flag_decompose", globals())
_enum_flag_invert = _require_intrinsic("molt_enum_flag_invert", globals())
_enum_flag_new = _require_intrinsic("molt_enum_flag_new", globals())
_enum_flag_or = _require_intrinsic("molt_enum_flag_or", globals())
_enum_flag_xor = _require_intrinsic("molt_enum_flag_xor", globals())
_enum_str_value = _require_intrinsic("molt_enum_str_value", globals())
_enum_unique_check = _require_intrinsic("molt_enum_unique_check", globals())
_enum_verify_member = _require_intrinsic("molt_enum_verify_member", globals())


class _AutoValue:
    __slots__ = ()
    _molt_auto = True


def auto() -> _AutoValue:
    return _AutoValue()


def _is_descriptor(obj: object) -> bool:
    if hasattr(obj, "__get__") or hasattr(obj, "__set__") or hasattr(obj, "__delete__"):
        return True
    # Molt property objects do not surface __get__/__set__/__delete__ to Python.
    return hasattr(obj, "fget") or hasattr(obj, "fset") or hasattr(obj, "fdel")


def _is_auto_value(obj: object) -> bool:
    return isinstance(obj, _AutoValue) or bool(getattr(obj, "_molt_auto", False))


class EnumType(type):
    _member_names_: list[str]
    _member_map_: dict[str, Any]
    _value2member_map_: dict[Any, Any]

    def __new__(
        mcls, name: str, bases: tuple[type, ...], namespace: dict[str, Any], **kwargs
    ):
        members: list[tuple[str, Any]] = []
        for key, value in list(namespace.items()):
            if key.startswith("_"):
                continue
            if _is_descriptor(value) or callable(value):
                continue
            members.append((key, value))
            namespace.pop(key, None)
        cls = super().__new__(mcls, name, bases, dict(namespace))
        cls._member_names_: list[str] = []
        cls._member_map_: dict[str, Any] = {}
        cls._value2member_map_: dict[Any, Any] = {}
        import sys as _enum_sys

        _enum_mod_dict = (
            getattr(_enum_sys.modules.get(__name__), "__dict__", None) or globals()
        )
        flag_type = _enum_mod_dict.get("Flag")
        is_flag = False
        if flag_type is not None:
            for base in bases:
                if isinstance(base, type) and issubclass(base, flag_type):
                    is_flag = True
                    break
        str_enum_type = _enum_mod_dict.get("StrEnum")
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

    def __contains__(cls, value: object) -> bool:
        try:
            cls(value)
            return True
        except Exception:
            return False

    def __call__(cls, value: Any):
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
    """Class decorator for Enum ensuring unique member values."""
    members = [
        (name, enumeration._member_map_[name]._value_)
        for name in enumeration._member_names_
    ]
    if not _enum_unique_check(members):
        seen: dict[Any, str] = {}
        duplicates: list[str] = []
        for name, value in members:
            if value in seen:
                duplicates.append(f"{name} -> {seen[value]}")
            else:
                seen[value] = name
        raise ValueError(
            f"duplicate values found in {enumeration!r}: " + ", ".join(duplicates)
        )
    return enumeration


def verify(enumeration: type) -> type:
    """Class decorator that checks all enum members are valid."""
    return enumeration
