"""Intrinsic-backed signal module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import enum as _enum

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_SIGNAL_RAISE = _require_intrinsic("molt_signal_raise", globals())
_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted", globals())
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())

# Signal constants from Rust intrinsics
_MOLT_SIGNAL_SIG_DFL = _require_intrinsic("molt_signal_sig_dfl", globals())
_MOLT_SIGNAL_SIG_IGN = _require_intrinsic("molt_signal_sig_ign", globals())
_MOLT_SIGNAL_SIGINT = _require_intrinsic("molt_signal_sigint", globals())
_MOLT_SIGNAL_SIGTERM = _require_intrinsic("molt_signal_sigterm", globals())
_MOLT_SIGNAL_SIGHUP = _require_intrinsic("molt_signal_sighup", globals())
_MOLT_SIGNAL_SIGQUIT = _require_intrinsic("molt_signal_sigquit", globals())
_MOLT_SIGNAL_SIGABRT = _require_intrinsic("molt_signal_sigabrt", globals())
_MOLT_SIGNAL_SIGFPE = _require_intrinsic("molt_signal_sigfpe", globals())
_MOLT_SIGNAL_SIGILL = _require_intrinsic("molt_signal_sigill", globals())
_MOLT_SIGNAL_SIGSEGV = _require_intrinsic("molt_signal_sigsegv", globals())
_MOLT_SIGNAL_SIGPIPE = _require_intrinsic("molt_signal_sigpipe", globals())
_MOLT_SIGNAL_SIGALRM = _require_intrinsic("molt_signal_sigalrm", globals())
_MOLT_SIGNAL_SIGUSR1 = _require_intrinsic("molt_signal_sigusr1", globals())
_MOLT_SIGNAL_SIGUSR2 = _require_intrinsic("molt_signal_sigusr2", globals())
_MOLT_SIGNAL_SIGCHLD = _require_intrinsic("molt_signal_sigchld", globals())
_MOLT_SIGNAL_NSIG = _require_intrinsic("molt_signal_nsig", globals())
_MOLT_SIGNAL_SIGNAL = _require_intrinsic("molt_signal_signal", globals())
_MOLT_SIGNAL_GETSIGNAL = _require_intrinsic("molt_signal_getsignal", globals())
_MOLT_SIGNAL_RAISE_SIGNAL = _require_intrinsic("molt_signal_raise_signal", globals())
_MOLT_SIGNAL_ALARM = _require_intrinsic("molt_signal_alarm", globals())
_MOLT_SIGNAL_PAUSE = _require_intrinsic("molt_signal_pause", globals())
_MOLT_SIGNAL_SET_WAKEUP_FD = _require_intrinsic("molt_signal_set_wakeup_fd", globals())
_MOLT_SIGNAL_VALID_SIGNALS = _require_intrinsic("molt_signal_valid_signals", globals())

# Signal number constants
SIG_DFL = int(_MOLT_SIGNAL_SIG_DFL())
SIG_IGN = int(_MOLT_SIGNAL_SIG_IGN())

SIGINT = int(_MOLT_SIGNAL_SIGINT())
SIGTERM = int(_MOLT_SIGNAL_SIGTERM())
SIGHUP = int(_MOLT_SIGNAL_SIGHUP())
SIGQUIT = int(_MOLT_SIGNAL_SIGQUIT())
SIGABRT = int(_MOLT_SIGNAL_SIGABRT())
SIGFPE = int(_MOLT_SIGNAL_SIGFPE())
SIGILL = int(_MOLT_SIGNAL_SIGILL())
SIGSEGV = int(_MOLT_SIGNAL_SIGSEGV())
SIGPIPE = int(_MOLT_SIGNAL_SIGPIPE())
SIGALRM = int(_MOLT_SIGNAL_SIGALRM())
SIGUSR1 = int(_MOLT_SIGNAL_SIGUSR1())
SIGUSR2 = int(_MOLT_SIGNAL_SIGUSR2())
SIGCHLD = int(_MOLT_SIGNAL_SIGCHLD())

NSIG = int(_MOLT_SIGNAL_NSIG())

__all__ = [
    "NSIG",
    "SIGABRT",
    "SIGALRM",
    "SIGCHLD",
    "SIGFPE",
    "SIGHUP",
    "SIGILL",
    "SIGINT",
    "SIGPIPE",
    "SIGQUIT",
    "SIGSEGV",
    "SIGTERM",
    "SIGUSR1",
    "SIGUSR2",
    "SIG_DFL",
    "SIG_IGN",
    "Signals",
    "alarm",
    "default_int_handler",
    "getsignal",
    "pause",
    "raise_signal",
    "set_wakeup_fd",
    "signal",
    "valid_signals",
]


def _require_cap() -> None:
    if _MOLT_CAPABILITIES_TRUSTED():
        return
    _MOLT_CAPABILITIES_REQUIRE("process.signal")


class Signals(_enum.IntEnum):
    SIGINT = SIGINT
    SIGTERM = SIGTERM
    SIGHUP = SIGHUP
    SIGQUIT = SIGQUIT
    SIGABRT = SIGABRT
    SIGFPE = SIGFPE
    SIGILL = SIGILL
    SIGSEGV = SIGSEGV
    SIGPIPE = SIGPIPE
    SIGALRM = SIGALRM
    SIGUSR1 = SIGUSR1
    SIGUSR2 = SIGUSR2
    SIGCHLD = SIGCHLD


def default_int_handler(_signum: int, _frame: object | None = None) -> None:
    raise KeyboardInterrupt


def getsignal(sig: int) -> object:
    _require_cap()
    return _MOLT_SIGNAL_GETSIGNAL(int(sig))


def signal(sig: int, handler: object) -> object:
    _require_cap()
    return _MOLT_SIGNAL_SIGNAL(int(sig), handler)


def raise_signal(sig: int) -> None:
    _require_cap()
    _MOLT_SIGNAL_RAISE_SIGNAL(int(sig))


def alarm(seconds: int) -> int:
    _require_cap()
    return int(_MOLT_SIGNAL_ALARM(int(seconds)))


def pause() -> None:
    _require_cap()
    _MOLT_SIGNAL_PAUSE()


def set_wakeup_fd(fd: int) -> int:
    _require_cap()
    return int(_MOLT_SIGNAL_SET_WAKEUP_FD(int(fd)))


def valid_signals() -> set[int]:
    _require_cap()
    result = _MOLT_SIGNAL_VALID_SIGNALS()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)
