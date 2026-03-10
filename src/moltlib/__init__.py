"""Molt-specific library modules outside the CPython stdlib namespace."""

from __future__ import annotations

from typing import Any
import importlib

__all__ = ["molt_db"]


def __getattr__(name: str) -> Any:
    if name != "molt_db":
        raise AttributeError(name)
    module = importlib.import_module("moltlib.molt_db")
    globals()[name] = module
    return module


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(globals()))
