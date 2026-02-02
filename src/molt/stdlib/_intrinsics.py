"""Intrinsic resolution helpers for Molt stdlib modules."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from molt import intrinsics as _intrinsics


def load_intrinsic(name: str, namespace: Mapping[str, Any]) -> Any | None:
    return _intrinsics.load(name, namespace)
