"""Molt-specific library modules outside the CPython stdlib namespace."""

from __future__ import annotations

from typing import Any
import importlib

__all__ = ["asgi", "concurrency", "io", "molt_db", "net"]

_LAZY_SUBMODULES = frozenset(__all__)


def __getattr__(name: str) -> Any:
    if name not in _LAZY_SUBMODULES:
        raise AttributeError(name)
    module = importlib.import_module(f"moltlib.{name}")
    globals()[name] = module
    return module


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(globals()))
