"""Minimal glob support for Molt."""

from __future__ import annotations

from molt.stdlib import fnmatch
from molt.stdlib import os

__all__ = ["glob", "iglob", "has_magic"]


_MAGIC_CHARS = "*?[]"


def has_magic(pathname: str) -> bool:
    return any(ch in pathname for ch in _MAGIC_CHARS)


def _iterdir(dirname: str) -> list[str]:
    return list(os.listdir(dirname))


def glob(pathname: str) -> list[str]:
    return list(iglob(pathname))


def iglob(pathname: str):
    dirname, basename = os.path.split(pathname)
    if not dirname:
        dirname = os.curdir
    if not has_magic(basename):
        if os.path.exists(pathname):
            yield pathname
        return
    for name in _iterdir(dirname):
        if fnmatch.fnmatch(name, basename):
            yield os.path.join(dirname, name)
