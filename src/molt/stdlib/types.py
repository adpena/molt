"""Minimal types helpers for Molt."""

from __future__ import annotations

from typing import Any, Iterable, Callable

import sys as _sys

__all__ = [
    "AsyncGeneratorType",
    "CodeType",
    "CoroutineType",
    "FrameType",
    "FunctionType",
    "GeneratorType",
    "MappingProxyType",
    "MethodType",
    "ModuleType",
    "NotImplementedType",
    "GenericAlias",
    "SimpleNamespace",
    "TracebackType",
    "UnionType",
    "coroutine",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): finish
# types helpers parity (DescriptorType, CellType, SimpleNamespace full API, etc).

NotImplementedType = type(NotImplemented)
GenericAlias = type(list[int])
UnionType = type(int | str)
ModuleType = type(_sys)
FunctionType = type(lambda: None)
CodeType = type((lambda: None).__code__)
FrameType = type(_sys._getframe())


def _traceback_type() -> type:
    try:
        1 / 0
    except Exception as exc:  # pragma: no cover - deterministic
        return type(exc.__traceback__)
    return type(None)


TracebackType = _traceback_type()


def _generator_type() -> type:
    def _gen() -> Any:
        yield None

    return type(_gen())


GeneratorType = _generator_type()


class _InstanceCheckMeta(type):
    def __instancecheck__(cls, instance: Any) -> bool:  # type: ignore[override]
        check = getattr(cls, "_check", None)
        if check is None:
            return False
        return bool(check(instance))


class CoroutineType(metaclass=_InstanceCheckMeta):
    @staticmethod
    def _check(instance: Any) -> bool:
        return hasattr(instance, "__await__")


class AsyncGeneratorType(metaclass=_InstanceCheckMeta):
    @staticmethod
    def _check(instance: Any) -> bool:
        return hasattr(instance, "__anext__") and hasattr(instance, "asend")


def _bound_method_type() -> type:
    class _Tmp:
        def method(self) -> None:
            return None

    return type(_Tmp().method)


_BOUND_METHOD_TYPE = _bound_method_type()


class _MethodTypeMeta(type):
    def __call__(cls, func: Callable[..., Any], obj: Any, cls_type: Any = None) -> Any:  # type: ignore[override]
        if obj is None:
            return func

        def bound(*args: Any, **kwargs: Any) -> Any:
            return func(obj, *args, **kwargs)

        bound.__name__ = getattr(func, "__name__", "method")
        bound.__qualname__ = getattr(func, "__qualname__", bound.__name__)
        bound.__doc__ = getattr(func, "__doc__", None)
        return bound

    def __instancecheck__(cls, instance: Any) -> bool:  # type: ignore[override]
        return type(instance) is _BOUND_METHOD_TYPE


class MethodType(metaclass=_MethodTypeMeta):
    pass


_is_coroutine = object()


class SimpleNamespace:
    def __init__(self, mapping: dict[str, Any] | None = None, /, **kwargs: Any) -> None:
        if mapping is not None:
            if not isinstance(mapping, dict):
                raise TypeError("mapping must be a dict")
            for key, val in mapping.items():
                setattr(self, key, val)
        for key, val in kwargs.items():
            setattr(self, key, val)

    def __repr__(self) -> str:
        items = list(self.__dict__.items())
        for idx in range(1, len(items)):
            current = items[idx]
            pos = idx - 1
            while pos >= 0 and items[pos][0] > current[0]:
                items[pos + 1] = items[pos]
                pos -= 1
            items[pos + 1] = current
        if not items:
            return "namespace()"
        parts_list: list[str] = []
        for item in items:
            key = item[0]
            val = item[1]
            parts_list.append(str(key) + "=" + repr(val))
        parts = ", ".join(parts_list)
        return "namespace(" + parts + ")"

    def __eq__(self, other: Any) -> bool:
        return self.__dict__ == other.__dict__


def coroutine(func: Callable[..., Any]) -> Callable[..., Any]:
    if hasattr(func, "__is_coroutine__"):
        return func

    def wrapper(*args: Any, **kwargs: Any) -> Any:
        return func(*args, **kwargs)

    wrapper.__is_coroutine__ = _is_coroutine  # type: ignore[attr-defined]
    wrapper.__name__ = getattr(func, "__name__", "coroutine")
    wrapper.__qualname__ = getattr(func, "__qualname__", wrapper.__name__)
    wrapper.__doc__ = getattr(func, "__doc__", None)
    return wrapper


class MappingProxyType:
    def __init__(self, mapping: dict[Any, Any]) -> None:
        self._mapping = mapping

    def __getitem__(self, key: Any) -> Any:
        return self._mapping[key]

    def __iter__(self) -> Iterable[Any]:
        return iter(self._mapping)

    def __len__(self) -> int:
        return len(self._mapping)

    def __contains__(self, key: Any) -> bool:
        return key in self._mapping

    def get(self, key: Any, default: Any = None) -> Any:
        return self._mapping.get(key, default)

    def keys(self) -> Any:
        return self._mapping.keys()

    def items(self) -> Any:
        return self._mapping.items()

    def values(self) -> Any:
        return self._mapping.values()
