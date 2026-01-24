"""Minimal types helpers for Molt."""

from __future__ import annotations

import sys as _sys
from typing import Any, Iterable

__all__ = [
    "SimpleNamespace",
    "MappingProxyType",
    "NotImplementedType",
    "GenericAlias",
    "ModuleType",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): add full types helpers (TracebackType, FrameType, FunctionType, MethodType, etc).

NotImplementedType = type(NotImplemented)
GenericAlias = type(list[int])
ModuleType = type(_sys)


class SimpleNamespace:
    def __init__(self, mapping: dict[str, Any] | None = None) -> None:
        if mapping is None:
            return
        for item in mapping.items():
            key = item[0]
            val = item[1]
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
