"""Byte-compilation utilities.

This is a minimal, deterministic subset that validates source availability.
"""

from __future__ import annotations

import os
import sys

__all__ = ["compile_file", "compile_dir", "compile_path"]


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement pyc generation, invalidation modes, and full compileall/py_compile parity.


def compile_file(
    fullname: str,
    ddir: str | None = None,
    force: bool = False,
    rx=None,
    quiet: int = 0,
    legacy: bool = False,
    optimize: int = -1,
    invalidation_mode=None,
    stripdir: str | None = None,
    prependdir: str | None = None,
    limit_sl_dest: int | None = None,
    worker=None,
    **_kwargs,
) -> bool:
    del (
        ddir,
        force,
        rx,
        legacy,
        optimize,
        invalidation_mode,
        stripdir,
        prependdir,
        limit_sl_dest,
        worker,
    )
    try:
        with open(fullname, "rb") as handle:
            handle.read(1)
    except OSError:
        return False
    return True


def compile_dir(
    dir: str,
    maxlevels: int = 10,
    ddir: str | None = None,
    force: bool = False,
    rx=None,
    quiet: int = 0,
    legacy: bool = False,
    optimize: int = -1,
    workers: int = 1,
    invalidation_mode=None,
    stripdir: str | None = None,
    prependdir: str | None = None,
    limit_sl_dest: int | None = None,
    **_kwargs,
) -> bool:
    del (
        ddir,
        force,
        rx,
        legacy,
        optimize,
        workers,
        invalidation_mode,
        stripdir,
        prependdir,
        limit_sl_dest,
    )
    try:
        entries = os.listdir(dir)
    except OSError:
        return False

    success = True
    for entry in entries:
        if entry == "__pycache__":
            continue
        full = _path_join(dir, entry)
        if entry.endswith(".py"):
            if not compile_file(full, quiet=quiet):
                success = False
            continue
        if maxlevels <= 0:
            continue
        try:
            os.listdir(full)
        except OSError:
            continue
        if not compile_dir(
            full,
            maxlevels=maxlevels - 1,
            quiet=quiet,
        ):
            success = False
    return success


def compile_path(
    skip_curdir: bool = True,
    maxlevels: int = 0,
    quiet: int = 0,
    **_kwargs,
) -> bool:
    success = True
    for entry in sys.path:
        if skip_curdir and entry in ("", "."):
            continue
        if not compile_dir(entry, maxlevels=maxlevels, quiet=quiet):
            success = False
    return success


def _path_join(base: str, name: str) -> str:
    if not base:
        return name
    sep = os.sep
    if base.endswith(sep):
        return base + name
    return base + sep + name
