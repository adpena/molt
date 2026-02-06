"""Minimal enum support for Molt."""

from __future__ import annotations

from typing import Any

__all__ = ["Enum", "IntEnum", "IntFlag", "Flag", "auto"]


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


class EnumMeta(type):
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
        flag_type = globals().get("Flag")
        is_flag = False
        if flag_type is not None:
            for base in bases:
                if isinstance(base, type) and issubclass(base, flag_type):
                    is_flag = True
                    break
        auto_value = 0
        flag_value = 1
        for member_name, raw_value in members:
            if _is_auto_value(raw_value):
                if is_flag:
                    raw_value = flag_value
                    flag_value <<= 1
                else:
                    auto_value += 1
                    raw_value = auto_value
            value = raw_value
            member = cls.__new__(cls, value)
            member._name_ = member_name
            member._value_ = value
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


class Enum(metaclass=EnumMeta):
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
    def __or__(self, other: Any):
        if isinstance(other, Flag):
            other_val = int(other._value_)
        else:
            other_val = int(other)
        return self.__class__(int(self._value_) | other_val)

    def __and__(self, other: Any):
        if isinstance(other, Flag):
            other_val = int(other._value_)
        else:
            other_val = int(other)
        return self.__class__(int(self._value_) & other_val)


class IntFlag(int, Flag):
    def __new__(cls, value: Any):
        obj = int.__new__(cls, int(value))
        obj._value_ = int(value)
        return obj
