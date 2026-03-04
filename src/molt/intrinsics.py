"""Intrinsic registry and lookup helpers for Molt."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

import _intrinsics as _loader


def runtime_active() -> bool:
    probe = getattr(_loader, "runtime_active", None)
    if callable(probe):
        return bool(probe())
    return False


def register(_name: str, _value: Any) -> None:
    raise RuntimeError("intrinsics registry is runtime-owned")


def load(name: str, namespace: Mapping[str, Any] | None = None) -> Any | None:
    return _loader.load_intrinsic(name, namespace)


def require(name: str, namespace: Mapping[str, Any] | None = None) -> Any:
    return _loader.require_intrinsic(name, namespace)
