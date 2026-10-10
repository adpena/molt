"""Minimal `tracemalloc` subset for Molt."""

from __future__ import annotations


_TRACING = False


def start(_nframe: int = 1) -> None:
    global _TRACING
    _TRACING = True


def stop() -> None:
    global _TRACING
    _TRACING = False


def is_tracing() -> bool:
    return _TRACING


def get_traced_memory() -> tuple[int, int]:
    return (0, 0)


__all__ = ["start", "stop", "is_tracing", "get_traced_memory"]
