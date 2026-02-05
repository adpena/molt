"""Minimal py_compile stub for deterministic environments."""

from __future__ import annotations

import os

__all__ = ["PyCompileError", "compile"]


class PyCompileError(Exception):
    pass


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full py_compile parity (pyc headers, invalidation modes, optimize levels).


def compile(
    file: str,
    cfile: str | None = None,
    dfile: str | None = None,
    doraise: bool = False,
    optimize: int = -1,
    invalidation_mode=None,
    quiet: int = 0,
    **_kwargs,
) -> str:
    del dfile, optimize, invalidation_mode, quiet
    try:
        with open(file, "rb") as handle:
            handle.read(1)
    except OSError as exc:
        if doraise:
            raise PyCompileError(str(exc))
        return ""

    if cfile is None:
        cfile = file + "c"

    try:
        with open(cfile, "wb") as handle:
            handle.write(b"")
    except OSError as exc:
        if doraise:
            raise PyCompileError(str(exc))
        return ""

    return os.path.abspath(cfile)
