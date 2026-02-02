"""Minimal stat constants for Molt."""

from __future__ import annotations

__all__ = ["S_IFDIR", "S_IFREG", "S_IFMT", "S_ISDIR", "S_ISREG"]

S_IFMT = 0o170000
S_IFDIR = 0o040000
S_IFREG = 0o100000


def S_ISDIR(mode: int) -> bool:
    return (mode & S_IFMT) == S_IFDIR


def S_ISREG(mode: int) -> bool:
    return (mode & S_IFMT) == S_IFREG
