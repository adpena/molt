"""Intrinsic registry and lookup helpers for Molt."""

from __future__ import annotations

import builtins as _builtins

TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any, Mapping
else:
    Any = object()  # type: ignore[assignment]
    Mapping = object()  # type: ignore[assignment]

_REGISTRY_NAME = "_molt_intrinsics"
_RUNTIME_FLAG = "_molt_runtime"
_STRICT_FLAG = "_molt_intrinsics_strict"


def runtime_active() -> bool:
    return bool(
        getattr(_builtins, _RUNTIME_FLAG, False)
        or getattr(_builtins, _STRICT_FLAG, False)
    )


def _registry() -> dict[str, Any] | None:
    if not runtime_active():
        return None
    reg = getattr(_builtins, _REGISTRY_NAME, None)
    if isinstance(reg, dict):
        return reg
    return None


def register(_name: str, _value: Any) -> None:
    raise RuntimeError("intrinsics registry is runtime-owned")


def load(name: str, namespace: Mapping[str, Any] | None = None) -> Any | None:
    if not runtime_active():
        return None
    if namespace is not None:
        value = namespace.get(name)
        if value is not None:
            return value
    reg = _registry()
    if reg is None:
        return None
    value = reg.get(name)
    if value is not None:
        return value
    return None


def require(name: str, namespace: Mapping[str, Any] | None = None) -> Any:
    value = load(name, namespace)
    if value is None:
        if not runtime_active():
            raise RuntimeError("Molt runtime intrinsics unavailable (runtime inactive)")
        raise RuntimeError(f"intrinsic unavailable: {name}")
    return value
