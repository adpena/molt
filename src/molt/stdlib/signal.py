"""Intrinsic-backed signal module for Molt."""

from __future__ import annotations

from collections.abc import Iterable as _Iterable
from _intrinsics import require_intrinsic as _require_intrinsic

import enum as _enum


# Signal constants from Rust intrinsics
_MOLT_SIGNAL_SIG_DFL = _require_intrinsic("molt_signal_sig_dfl")
_MOLT_SIGNAL_SIG_IGN = _require_intrinsic("molt_signal_sig_ign")
_MOLT_SIGNAL_SIGINT = _require_intrinsic("molt_signal_sigint")
_MOLT_SIGNAL_SIGTERM = _require_intrinsic("molt_signal_sigterm")
_MOLT_SIGNAL_SIGHUP = _require_intrinsic("molt_signal_sighup")
_MOLT_SIGNAL_SIGQUIT = _require_intrinsic("molt_signal_sigquit")
_MOLT_SIGNAL_SIGABRT = _require_intrinsic("molt_signal_sigabrt")
_MOLT_SIGNAL_SIGFPE = _require_intrinsic("molt_signal_sigfpe")
_MOLT_SIGNAL_SIGILL = _require_intrinsic("molt_signal_sigill")
_MOLT_SIGNAL_SIGSEGV = _require_intrinsic("molt_signal_sigsegv")
_MOLT_SIGNAL_SIGPIPE = _require_intrinsic("molt_signal_sigpipe")
_MOLT_SIGNAL_SIGALRM = _require_intrinsic("molt_signal_sigalrm")
_MOLT_SIGNAL_SIGUSR1 = _require_intrinsic("molt_signal_sigusr1")
_MOLT_SIGNAL_SIGUSR2 = _require_intrinsic("molt_signal_sigusr2")
_MOLT_SIGNAL_SIGCHLD = _require_intrinsic("molt_signal_sigchld")
_MOLT_SIGNAL_NSIG = _require_intrinsic("molt_signal_nsig")
_MOLT_SIGNAL_SIG_BLOCK = _require_intrinsic("molt_signal_sig_block")
_MOLT_SIGNAL_SIG_UNBLOCK = _require_intrinsic("molt_signal_sig_unblock")
_MOLT_SIGNAL_SIG_SETMASK = _require_intrinsic("molt_signal_sig_setmask")
_MOLT_SIGNAL_SIGNAL = _require_intrinsic("molt_signal_signal")
_MOLT_SIGNAL_GETSIGNAL = _require_intrinsic("molt_signal_getsignal")
_MOLT_SIGNAL_VALID_SIGNALS = _require_intrinsic("molt_signal_valid_signals")

# New signal constant intrinsics
_MOLT_SIGNAL_SIGBUS = _require_intrinsic("molt_signal_sigbus")
_MOLT_SIGNAL_SIGCONT = _require_intrinsic("molt_signal_sigcont")
_MOLT_SIGNAL_SIGSTOP = _require_intrinsic("molt_signal_sigstop")
_MOLT_SIGNAL_SIGTSTP = _require_intrinsic("molt_signal_sigtstp")
_MOLT_SIGNAL_SIGTTIN = _require_intrinsic("molt_signal_sigttin")
_MOLT_SIGNAL_SIGTTOU = _require_intrinsic("molt_signal_sigttou")
_MOLT_SIGNAL_SIGXCPU = _require_intrinsic("molt_signal_sigxcpu")
_MOLT_SIGNAL_SIGXFSZ = _require_intrinsic("molt_signal_sigxfsz")
_MOLT_SIGNAL_SIGVTALRM = _require_intrinsic("molt_signal_sigvtalrm")
_MOLT_SIGNAL_SIGPROF = _require_intrinsic("molt_signal_sigprof")
_MOLT_SIGNAL_SIGWINCH = _require_intrinsic("molt_signal_sigwinch")
_MOLT_SIGNAL_SIGSYS = _require_intrinsic("molt_signal_sigsys")

# New function intrinsics
_MOLT_SIGNAL_PTHREAD_SIGMASK = _require_intrinsic("molt_signal_pthread_sigmask")
_MOLT_SIGNAL_SIGPENDING = _require_intrinsic("molt_signal_sigpending")
_MOLT_SIGNAL_SIGWAIT = _require_intrinsic("molt_signal_sigwait")

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

# New signal constants
SIGBUS = int(_MOLT_SIGNAL_SIGBUS())
SIGCONT = int(_MOLT_SIGNAL_SIGCONT())
SIGSTOP = int(_MOLT_SIGNAL_SIGSTOP())
SIGTSTP = int(_MOLT_SIGNAL_SIGTSTP())
SIGTTIN = int(_MOLT_SIGNAL_SIGTTIN())
SIGTTOU = int(_MOLT_SIGNAL_SIGTTOU())
SIGXCPU = int(_MOLT_SIGNAL_SIGXCPU())
SIGXFSZ = int(_MOLT_SIGNAL_SIGXFSZ())
SIGVTALRM = int(_MOLT_SIGNAL_SIGVTALRM())
SIGPROF = int(_MOLT_SIGNAL_SIGPROF())
SIGWINCH = int(_MOLT_SIGNAL_SIGWINCH())
SIGSYS = int(_MOLT_SIGNAL_SIGSYS())

NSIG = int(_MOLT_SIGNAL_NSIG())

# POSIX sigmask how constants
SIG_BLOCK = int(_MOLT_SIGNAL_SIG_BLOCK())
SIG_UNBLOCK = int(_MOLT_SIGNAL_SIG_UNBLOCK())
SIG_SETMASK = int(_MOLT_SIGNAL_SIG_SETMASK())

__all__ = [
    "NSIG",
    "SIGABRT",
    "SIGALRM",
    "SIGBUS",
    "SIGCHLD",
    "SIGCONT",
    "SIGFPE",
    "SIGHUP",
    "SIGILL",
    "SIGINT",
    "SIGPIPE",
    "SIGPROF",
    "SIGQUIT",
    "SIGSEGV",
    "SIGSTOP",
    "SIGSYS",
    "SIGTERM",
    "SIGTSTP",
    "SIGTTIN",
    "SIGTTOU",
    "SIGUSR1",
    "SIGUSR2",
    "SIGVTALRM",
    "SIGWINCH",
    "SIGXCPU",
    "SIGXFSZ",
    "SIG_BLOCK",
    "SIG_DFL",
    "SIG_IGN",
    "SIG_SETMASK",
    "SIG_UNBLOCK",
    "Signals",
    "Handlers",
    "Sigmasks",
    "alarm",
    "default_int_handler",
    "getsignal",
    "pause",
    "pthread_kill",
    "pthread_sigmask",
    "raise_signal",
    "set_wakeup_fd",
    "sigpending",
    "signal",
    "sigwait",
    "strsignal",
    "valid_signals",
]


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
    SIGBUS = SIGBUS
    SIGCONT = SIGCONT
    SIGSTOP = SIGSTOP
    SIGTSTP = SIGTSTP
    SIGTTIN = SIGTTIN
    SIGTTOU = SIGTTOU
    SIGXCPU = SIGXCPU
    SIGXFSZ = SIGXFSZ
    SIGVTALRM = SIGVTALRM
    SIGPROF = SIGPROF
    SIGWINCH = SIGWINCH
    SIGSYS = SIGSYS

    def __str__(self) -> str:
        return str(int(self))


class Handlers(_enum.IntEnum):
    SIG_DFL = SIG_DFL
    SIG_IGN = SIG_IGN


class Sigmasks(_enum.IntEnum):
    SIG_BLOCK = SIG_BLOCK
    SIG_UNBLOCK = SIG_UNBLOCK
    SIG_SETMASK = SIG_SETMASK


for _enum_class in (Signals, Handlers, Sigmasks):
    for _member in _enum_class:
        globals()[_member.name] = _member


def _int_to_enum(value, enum_class):
    if not isinstance(value, int):
        return value
    try:
        return enum_class(value)
    except ValueError:
        return value


# BEGIN PUBLIC OBJECT molt_signal_default_int_handler
# Generated by tools/gen_intrinsics.py from public_objects.toml.
default_int_handler = _require_intrinsic('molt_signal_default_int_handler')
# END PUBLIC OBJECT molt_signal_default_int_handler


def getsignal(sig: int) -> object:
    # The runtime owns SIGINT's `default_int_handler` disposition.
    return _int_to_enum(_MOLT_SIGNAL_GETSIGNAL(int(sig)), Handlers)


def signal(sig: int, handler: object) -> object:
    return _int_to_enum(_MOLT_SIGNAL_SIGNAL(int(sig), handler), Handlers)



# BEGIN PUBLIC OBJECT molt_signal_raise_signal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import raise_signal
# END PUBLIC OBJECT molt_signal_raise_signal



# BEGIN PUBLIC OBJECT molt_signal_alarm
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import alarm
# END PUBLIC OBJECT molt_signal_alarm



# BEGIN PUBLIC OBJECT molt_signal_pause
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import pause
# END PUBLIC OBJECT molt_signal_pause



# BEGIN PUBLIC OBJECT molt_signal_set_wakeup_fd
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import set_wakeup_fd
# END PUBLIC OBJECT molt_signal_set_wakeup_fd


def valid_signals() -> set[int]:
    result = _MOLT_SIGNAL_VALID_SIGNALS()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)



# BEGIN PUBLIC OBJECT molt_signal_strsignal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import strsignal
# END PUBLIC OBJECT molt_signal_strsignal


def pthread_sigmask(how: int, mask: _Iterable[int]) -> set[int]:
    """Fetch and/or change the signal mask of the calling thread."""
    result = _MOLT_SIGNAL_PTHREAD_SIGMASK(int(how), list(mask))
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)



# BEGIN PUBLIC OBJECT molt_signal_pthread_kill
# Generated by tools/gen_intrinsics.py from public_objects.toml.
from _signal import pthread_kill
# END PUBLIC OBJECT molt_signal_pthread_kill


def sigpending() -> set[int]:
    """Examine pending signals."""
    result = _MOLT_SIGNAL_SIGPENDING()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def sigwait(sigset: _Iterable[int]) -> int:
    """Wait for a signal."""
    return int(_MOLT_SIGNAL_SIGWAIT(list(sigset)))


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's signal API.
# ---------------------------------------------------------------------------
for _name in ("_Iterable",):
    globals().pop(_name, None)

globals().pop("_require_intrinsic", None)
