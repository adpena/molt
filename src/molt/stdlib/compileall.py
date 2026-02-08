"""Intrinsic-backed compileall subset for Molt."""

from __future__ import annotations

import sys

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["compile_file", "compile_dir", "compile_path"]


_MOLT_COMPILEALL_COMPILE_FILE = _require_intrinsic(
    "molt_compileall_compile_file", globals()
)
_MOLT_COMPILEALL_COMPILE_DIR = _require_intrinsic(
    "molt_compileall_compile_dir", globals()
)
_MOLT_COMPILEALL_COMPILE_PATH = _require_intrinsic(
    "molt_compileall_compile_path", globals()
)


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
        quiet,
        legacy,
        optimize,
        invalidation_mode,
        stripdir,
        prependdir,
        limit_sl_dest,
        worker,
    )
    return bool(_MOLT_COMPILEALL_COMPILE_FILE(fullname))


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
        quiet,
        legacy,
        optimize,
        workers,
        invalidation_mode,
        stripdir,
        prependdir,
        limit_sl_dest,
    )
    return bool(_MOLT_COMPILEALL_COMPILE_DIR(dir, int(maxlevels)))


def compile_path(
    skip_curdir: bool = True,
    maxlevels: int = 0,
    quiet: int = 0,
    **_kwargs,
) -> bool:
    del quiet
    return bool(_MOLT_COMPILEALL_COMPILE_PATH(sys.path, skip_curdir, int(maxlevels)))
