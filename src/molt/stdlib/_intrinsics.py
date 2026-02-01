"""Intrinsic resolution helpers for Molt stdlib modules."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any
import builtins as _builtins


def load_intrinsic(name: str, namespace: Mapping[str, Any]) -> Any | None:
    direct = namespace.get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)
