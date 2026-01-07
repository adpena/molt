"""Minimal types helpers for Molt."""

from __future__ import annotations

from typing import Any, Iterable

__all__ = ["SimpleNamespace", "MappingProxyType"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): add full types helpers.


class SimpleNamespace:
    def __init__(self, mapping: dict[str, Any] | None = None) -> None:
        if mapping is None:
            return
        for item in mapping.items():
            key = item[0]
            val = item[1]
            setattr(self, key, val)

    def __repr__(self) -> str:
        items = sorted(self.__dict__.items())
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
