"""Intrinsic-backed `atexit` wrappers for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ATEXIT_REGISTER = _require_intrinsic("molt_atexit_register")
_MOLT_ATEXIT_UNREGISTER = _require_intrinsic("molt_atexit_unregister")
_MOLT_ATEXIT_CLEAR = _require_intrinsic("molt_atexit_clear")
_MOLT_ATEXIT_RUN_EXITFUNCS = _require_intrinsic("molt_atexit_run_exitfuncs")
_MOLT_ATEXIT_NCALLBACKS = _require_intrinsic("molt_atexit_ncallbacks")


def _normalize_no_args(
    name: str, args: tuple[Any, ...], kwargs: dict[str, Any]
) -> None:
    if kwargs:
        raise TypeError(f"atexit.{name}() takes no keyword arguments")
    arg_count = len(args)
    if arg_count:
        raise TypeError(f"atexit.{name}() takes no arguments ({arg_count} given)")


def register(*args: Any, **kwargs: Any) -> Any:
    if not args:
        raise TypeError("register() takes at least 1 argument (0 given)")
    func = args[0]
    if not callable(func):
        raise TypeError("the first argument must be callable")
    _MOLT_ATEXIT_REGISTER(func, tuple(args[1:]), dict(kwargs))
    return func


def unregister(*args: Any, **kwargs: Any) -> None:
    if kwargs:
        raise TypeError("atexit.unregister() takes no keyword arguments")
    arg_count = len(args)
    if arg_count != 1:
        raise TypeError(
            f"atexit.unregister() takes exactly one argument ({arg_count} given)"
        )
    _MOLT_ATEXIT_UNREGISTER(args[0])
    return None


def _clear(*args: Any, **kwargs: Any) -> None:
    _normalize_no_args("_clear", args, kwargs)
    _MOLT_ATEXIT_CLEAR()
    return None


def _run_exitfuncs(*args: Any, **kwargs: Any) -> None:
    _normalize_no_args("_run_exitfuncs", args, kwargs)
    _MOLT_ATEXIT_RUN_EXITFUNCS()
    return None


def _ncallbacks(*args: Any, **kwargs: Any) -> int:
    _normalize_no_args("_ncallbacks", args, kwargs)
    return int(_MOLT_ATEXIT_NCALLBACKS())


globals().pop("_require_intrinsic", None)
