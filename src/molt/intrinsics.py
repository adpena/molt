"""Intrinsic registry and lookup helpers for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING
import builtins as _builtins

if TYPE_CHECKING:
    from typing import Any, Iterable, Mapping

_REGISTRY_NAME = "_molt_intrinsics"


def _registry() -> dict[str, Any]:
    reg = getattr(_builtins, _REGISTRY_NAME, None)
    if reg is None:
        reg = {}
        try:
            setattr(_builtins, _REGISTRY_NAME, reg)
        except Exception:
            pass
    return reg


def register(name: str, value: Any) -> None:
    if value is None:
        return
    reg = _registry()
    if name not in reg:
        reg[name] = value
    try:
        existing = getattr(_builtins, name, None)
    except Exception:
        existing = None
    if existing is None:
        try:
            setattr(_builtins, name, value)
        except Exception:
            pass


def register_from_builtins(names: Iterable[str]) -> None:
    reg = _registry()
    for name in names:
        try:
            value = getattr(_builtins, name)
        except Exception:
            continue
        if value is None:
            continue
        if name not in reg:
            reg[name] = value


def load(name: str, namespace: Mapping[str, Any] | None = None) -> Any | None:
    if namespace is not None:
        value = namespace.get(name)
        if value is not None:
            return value
    reg = getattr(_builtins, _REGISTRY_NAME, None)
    if reg:
        value = reg.get(name)
        if value is not None:
            return value
    return getattr(_builtins, name, None)
