"""Minimal shutil support for Molt."""

from __future__ import annotations


from molt.stdlib import os

__all__ = ["copyfile", "which"]


def copyfile(src: str, dst: str) -> str:
    with open(src, "rb") as handle:
        data = handle.read()
    with open(dst, "wb") as handle:
        handle.write(data)
    return dst


def which(cmd: str, mode: int | None = None, path: str | None = None) -> str | None:
    del mode
    if path is None:
        path = os.environ.get("PATH", "")
    if os.path.isabs(cmd) and os.path.exists(cmd):
        return cmd
    for entry in path.split(os.pathsep):
        if not entry:
            continue
        candidate = os.path.join(entry, cmd)
        if os.path.exists(candidate):
            return candidate
    return None
