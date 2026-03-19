"""Intrinsic-backed _signal module for Molt.

This is the low-level C-extension-equivalent module that backs ``signal.py``.
In CPython this is implemented in ``Modules/signalmodule.c``; in Molt every
entry point is backed by a Rust intrinsic.

Capability gating is NOT performed here -- that is the responsibility of the
higher-level ``signal`` module.  The ``Signals`` IntEnum is likewise defined
only in ``signal.py``.
"""

from __future__ import annotations

from collections.abc import Iterable

from _intrinsics import require_intrinsic as _require_intrinsic

# -- probe intrinsic (standard pattern) ------------------------------------
_MOLT_STDLIB_PROBE = _require_intrinsic("molt_stdlib_probe")

# -- signal constant intrinsics --------------------------------------------
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

# -- function intrinsics ---------------------------------------------------
_MOLT_SIGNAL_SIGNAL = _require_intrinsic("molt_signal_signal")
_MOLT_SIGNAL_GETSIGNAL = _require_intrinsic("molt_signal_getsignal")
_MOLT_SIGNAL_RAISE_SIGNAL = _require_intrinsic("molt_signal_raise_signal")
_MOLT_SIGNAL_ALARM = _require_intrinsic("molt_signal_alarm")
_MOLT_SIGNAL_PAUSE = _require_intrinsic("molt_signal_pause")
_MOLT_SIGNAL_SET_WAKEUP_FD = _require_intrinsic("molt_signal_set_wakeup_fd")
_MOLT_SIGNAL_VALID_SIGNALS = _require_intrinsic("molt_signal_valid_signals")
_MOLT_SIGNAL_STRSIGNAL = _require_intrinsic("molt_signal_strsignal")
_MOLT_SIGNAL_PTHREAD_SIGMASK = _require_intrinsic(
    "molt_signal_pthread_sigmask"
)
_MOLT_SIGNAL_PTHREAD_KILL = _require_intrinsic("molt_signal_pthread_kill")
_MOLT_SIGNAL_SIGPENDING = _require_intrinsic("molt_signal_sigpending")
_MOLT_SIGNAL_SIGWAIT = _require_intrinsic("molt_signal_sigwait")
_MOLT_SIGNAL_DEFAULT_INT_HANDLER = _require_intrinsic(
    "molt_signal_default_int_handler"
)

# -- signal number constants -----------------------------------------------
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

# POSIX sigmask *how* constants
SIG_BLOCK = int(_MOLT_SIGNAL_SIG_BLOCK())
SIG_UNBLOCK = int(_MOLT_SIGNAL_SIG_UNBLOCK())
SIG_SETMASK = int(_MOLT_SIGNAL_SIG_SETMASK())

# ---------------------------------------------------------------------------
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
    "alarm",
    "default_int_handler",
    "getsignal",
    "pause",
    "pthread_kill",
    "pthread_sigmask",
    "raise_signal",
    "set_wakeup_fd",
    "signal",
    "sigpending",
    "sigwait",
    "strsignal",
    "valid_signals",
]


# ---------------------------------------------------------------------------
# Functions
# ---------------------------------------------------------------------------


default_int_handler = _MOLT_SIGNAL_DEFAULT_INT_HANDLER


def signal(sig: int, handler: object) -> object:
    """Set the action for the given signal."""
    signum = int(sig)
    old_handler = _MOLT_SIGNAL_SIGNAL(signum, handler)
    if signum == SIGINT and old_handler == SIG_DFL:
        return default_int_handler
    return old_handler


def getsignal(sig: int) -> object:
    """Return the current action for the given signal."""
    signum = int(sig)
    current = _MOLT_SIGNAL_GETSIGNAL(signum)
    if signum == SIGINT and current == SIG_DFL:
        return default_int_handler
    return current


def raise_signal(sig: int) -> None:
    """Send a signal to the calling process."""
    _MOLT_SIGNAL_RAISE_SIGNAL(int(sig))


def alarm(seconds: int) -> int:
    """Arrange for SIGALRM to arrive after the given number of seconds."""
    return int(_MOLT_SIGNAL_ALARM(int(seconds)))


def pause() -> None:
    """Wait until a signal is received."""
    _MOLT_SIGNAL_PAUSE()


def set_wakeup_fd(fd: int) -> int:
    """Set the file descriptor for the signal wakeup pipe."""
    return int(_MOLT_SIGNAL_SET_WAKEUP_FD(int(fd)))


def valid_signals() -> set[int]:
    """Return the set of valid signal numbers on this platform."""
    result = _MOLT_SIGNAL_VALID_SIGNALS()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def strsignal(signalnum: int) -> str | None:
    """Return the system description of the given signal."""
    result = _MOLT_SIGNAL_STRSIGNAL(int(signalnum))
    if result is None:
        return None
    return str(result)


def pthread_sigmask(how: int, mask: Iterable[int]) -> set[int]:
    """Fetch and/or change the signal mask of the calling thread."""
    result = _MOLT_SIGNAL_PTHREAD_SIGMASK(int(how), list(mask))
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def pthread_kill(thread_id: int, signalnum: int) -> None:
    """Send a signal to a thread."""
    _MOLT_SIGNAL_PTHREAD_KILL(int(thread_id), int(signalnum))


def sigpending() -> set[int]:
    """Examine pending signals."""
    result = _MOLT_SIGNAL_SIGPENDING()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)


def sigwait(sigset: Iterable[int]) -> int:
    """Wait for a signal."""
    return int(_MOLT_SIGNAL_SIGWAIT(list(sigset)))

globals().pop("_require_intrinsic", None)
