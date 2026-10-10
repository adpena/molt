"""Minimal `faulthandler` subset for Molt."""

from __future__ import annotations


def enable(*_args, **_kwargs) -> None:
    pass


def disable() -> None:
    pass


def is_enabled() -> bool:
    return False


__all__ = ["enable", "disable", "is_enabled"]
