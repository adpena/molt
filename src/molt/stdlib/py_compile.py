"""Intrinsic-backed py_compile subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["PyCompileError", "compile"]


_MOLT_PY_COMPILE_COMPILE = _require_intrinsic("molt_py_compile_compile", globals())


class PyCompileError(Exception):
    pass


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
        out = _MOLT_PY_COMPILE_COMPILE(file, cfile)
    except OSError as exc:
        if doraise:
            raise PyCompileError(str(exc)) from exc
        return ""
    if not isinstance(out, str):
        raise RuntimeError("py_compile.compile intrinsic returned invalid value")
    return out
