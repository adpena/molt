"""Intrinsic registry and lookup helpers for Molt."""

from __future__ import annotations

TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any, Mapping
else:
    Any = object()  # type: ignore[assignment]
    Mapping = object()  # type: ignore[assignment]

import _intrinsics as _loader


def runtime_active() -> bool:
    return bool(_loader.runtime_active())


def register(_name: str, _value: Any) -> None:
    raise RuntimeError("intrinsics registry is runtime-owned")


def load(name: str, namespace: Mapping[str, Any] | None = None) -> Any | None:
    return _loader.load_intrinsic(name, namespace)


def require(name: str, namespace: Mapping[str, Any] | None = None) -> Any:
    return _loader.require_intrinsic(name, namespace)
