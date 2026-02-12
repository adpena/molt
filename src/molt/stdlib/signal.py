"""Minimal signal support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import enum as _enum

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_SIGNAL_RAISE = _require_intrinsic("molt_signal_raise", globals())
_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted", globals())
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())

__all__ = [
    "SIGINT",
    "SIG_DFL",
    "SIG_IGN",
    "Signals",
    "default_int_handler",
    "getsignal",
    "raise_signal",
    "signal",
]


def _require_cap() -> None:
    if _MOLT_CAPABILITIES_TRUSTED():
        return
    _MOLT_CAPABILITIES_REQUIRE("process.signal")


SIGINT = 2


class _SigDefault:
    pass


class _SigIgnore:
    pass


SIG_DFL = _SigDefault()
SIG_IGN = _SigIgnore()


class Signals(_enum.IntEnum):
    SIGINT = SIGINT


def default_int_handler(_signum: int, _frame: object | None = None) -> None:
    raise KeyboardInterrupt


_handlers: dict[int, object] = {SIGINT: default_int_handler}


def getsignal(sig: int) -> object:
    _require_cap()
    return _handlers.get(int(sig), SIG_DFL)


def signal(sig: int, handler: object) -> object:
    _require_cap()
    sig_num = int(sig)
    prev = _handlers.get(sig_num, SIG_DFL)
    _handlers[sig_num] = handler
    return prev


def raise_signal(sig: int) -> None:
    _require_cap()
    sig_num = int(sig)
    handler = getsignal(sig_num)
    if handler is SIG_IGN:
        return
    if handler is SIG_DFL or handler is default_int_handler:
        _MOLT_SIGNAL_RAISE(sig_num)
        if sig_num == SIGINT:
            raise KeyboardInterrupt
        return
    if not callable(handler):
        raise TypeError("signal handler must be callable")
    handler(sig_num, None)
