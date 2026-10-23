"""Intrinsic-backed `atexit` wrappers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ATEXIT_REGISTER = _require_intrinsic("molt_atexit_register")
_MOLT_ATEXIT_UNREGISTER = _require_intrinsic("molt_atexit_unregister")
_MOLT_ATEXIT_CLEAR = _require_intrinsic("molt_atexit_clear")
_MOLT_ATEXIT_RUN_EXITFUNCS = _require_intrinsic("molt_atexit_run_exitfuncs")
_MOLT_ATEXIT_NCALLBACKS = _require_intrinsic("molt_atexit_ncallbacks")


def _normalize_no_args(
    name: str, args: tuple[object, ...], kwargs: dict[str, object]
) -> None:
    if kwargs:
        raise TypeError(f"atexit.{name}() takes no keyword arguments")
    arg_count = len(args)
    if arg_count:
        raise TypeError(f"atexit.{name}() takes no arguments ({arg_count} given)")


def register(*args: object, **kwargs: object) -> object:
    if not args:
        raise TypeError("register() takes at least 1 argument (0 given)")
    func = args[0]
    if not callable(func):
        raise TypeError("the first argument must be callable")
    _MOLT_ATEXIT_REGISTER(func, tuple(args[1:]), dict(kwargs))
    return func


def unregister(*args: object, **kwargs: object) -> None:
    if kwargs:
        raise TypeError("atexit.unregister() takes no keyword arguments")
    arg_count = len(args)
    if arg_count != 1:
        raise TypeError(
            f"atexit.unregister() takes exactly one argument ({arg_count} given)"
        )
    _MOLT_ATEXIT_UNREGISTER(args[0])
    return None


def _clear(*args: object, **kwargs: object) -> None:
    _normalize_no_args("_clear", args, kwargs)
    _MOLT_ATEXIT_CLEAR()
    return None


def _run_exitfuncs(*args: object, **kwargs: object) -> None:
    _normalize_no_args("_run_exitfuncs", args, kwargs)
    _MOLT_ATEXIT_RUN_EXITFUNCS()
    return None


def _ncallbacks(*args: object, **kwargs: object) -> int:
    _normalize_no_args("_ncallbacks", args, kwargs)
    return int(_MOLT_ATEXIT_NCALLBACKS())


globals().pop("_require_intrinsic", None)
