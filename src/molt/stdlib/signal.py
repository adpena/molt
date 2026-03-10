"""Intrinsic-backed signal module for Molt."""

from __future__ import annotations

from collections.abc import Iterable
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
_MOLT_SIGNAL_SIG_BLOCK = _require_intrinsic("molt_signal_sig_block", globals())
_MOLT_SIGNAL_SIG_UNBLOCK = _require_intrinsic("molt_signal_sig_unblock", globals())
_MOLT_SIGNAL_SIG_SETMASK = _require_intrinsic("molt_signal_sig_setmask", globals())
_MOLT_SIGNAL_SIGNAL = _require_intrinsic("molt_signal_signal", globals())
_MOLT_SIGNAL_GETSIGNAL = _require_intrinsic("molt_signal_getsignal", globals())
_MOLT_SIGNAL_RAISE_SIGNAL = _require_intrinsic("molt_signal_raise_signal", globals())
_MOLT_SIGNAL_ALARM = _require_intrinsic("molt_signal_alarm", globals())
_MOLT_SIGNAL_PAUSE = _require_intrinsic("molt_signal_pause", globals())
_MOLT_SIGNAL_SET_WAKEUP_FD = _require_intrinsic("molt_signal_set_wakeup_fd", globals())
_MOLT_SIGNAL_VALID_SIGNALS = _require_intrinsic("molt_signal_valid_signals", globals())

# New signal constant intrinsics
_MOLT_SIGNAL_SIGBUS = _require_intrinsic("molt_signal_sigbus", globals())
_MOLT_SIGNAL_SIGCONT = _require_intrinsic("molt_signal_sigcont", globals())
_MOLT_SIGNAL_SIGSTOP = _require_intrinsic("molt_signal_sigstop", globals())
_MOLT_SIGNAL_SIGTSTP = _require_intrinsic("molt_signal_sigtstp", globals())
_MOLT_SIGNAL_SIGTTIN = _require_intrinsic("molt_signal_sigttin", globals())
_MOLT_SIGNAL_SIGTTOU = _require_intrinsic("molt_signal_sigttou", globals())
_MOLT_SIGNAL_SIGXCPU = _require_intrinsic("molt_signal_sigxcpu", globals())
_MOLT_SIGNAL_SIGXFSZ = _require_intrinsic("molt_signal_sigxfsz", globals())
_MOLT_SIGNAL_SIGVTALRM = _require_intrinsic("molt_signal_sigvtalrm", globals())
_MOLT_SIGNAL_SIGPROF = _require_intrinsic("molt_signal_sigprof", globals())
_MOLT_SIGNAL_SIGWINCH = _require_intrinsic("molt_signal_sigwinch", globals())
_MOLT_SIGNAL_SIGSYS = _require_intrinsic("molt_signal_sigsys", globals())

# New function intrinsics
_MOLT_SIGNAL_STRSIGNAL = _require_intrinsic("molt_signal_strsignal", globals())
_MOLT_SIGNAL_PTHREAD_SIGMASK = _require_intrinsic(
    "molt_signal_pthread_sigmask", globals()
)
_MOLT_SIGNAL_PTHREAD_KILL = _require_intrinsic("molt_signal_pthread_kill", globals())
_MOLT_SIGNAL_SIGPENDING = _require_intrinsic("molt_signal_sigpending", globals())
_MOLT_SIGNAL_SIGWAIT = _require_intrinsic("molt_signal_sigwait", globals())
_MOLT_SIGNAL_DEFAULT_INT_HANDLER = _require_intrinsic(
    "molt_signal_default_int_handler", globals()
)

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


default_int_handler = _MOLT_SIGNAL_DEFAULT_INT_HANDLER


def getsignal(sig: int) -> object:
    _require_cap()
    signum = int(sig)
    current = _MOLT_SIGNAL_GETSIGNAL(signum)
    if signum == SIGINT and current == SIG_DFL:
        return default_int_handler
    return current


def signal(sig: int, handler: object) -> object:
    _require_cap()
    signum = int(sig)
    old_handler = _MOLT_SIGNAL_SIGNAL(signum, handler)
    if signum == SIGINT and old_handler == SIG_DFL:
        return default_int_handler
    return old_handler


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


def strsignal(signalnum: int) -> str | None:
    """Return the system description of the given signal."""
    _require_cap()
    result = _MOLT_SIGNAL_STRSIGNAL(int(signalnum))
    if result is None:
        return None
    return str(result)


def pthread_sigmask(how: int, mask: Iterable[int]) -> set[int]:
    """Fetch and/or change the signal mask of the calling thread."""
    _require_cap()
    result = _MOLT_SIGNAL_PTHREAD_SIGMASK(int(how), list(mask))
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def pthread_kill(thread_id: int, signalnum: int) -> None:
    """Send a signal to a thread."""
    _require_cap()
    _MOLT_SIGNAL_PTHREAD_KILL(int(thread_id), int(signalnum))


def sigpending() -> set[int]:
    """Examine pending signals."""
    _require_cap()
    result = _MOLT_SIGNAL_SIGPENDING()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def sigwait(sigset: Iterable[int]) -> int:
    """Wait for a signal."""
    _require_cap()
    return int(_MOLT_SIGNAL_SIGWAIT(list(sigset)))


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's signal API.
# ---------------------------------------------------------------------------
for _name in ("Iterable",):
    globals().pop(_name, None)
