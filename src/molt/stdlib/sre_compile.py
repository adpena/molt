"""Minimal `sre_compile` subset for Molt."""

from __future__ import annotations


def compile(_pattern, _flags: int = 0):
    return None


__all__ = ["compile"]
