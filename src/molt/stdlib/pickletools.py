"""Minimal `pickletools` subset for Molt."""

from __future__ import annotations


def optimize(data: bytes) -> bytes:
    return bytes(data)


__all__ = ["optimize"]
