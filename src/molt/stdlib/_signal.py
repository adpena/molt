"""Intrinsic-backed _signal module for Molt.

This is the low-level C-extension-equivalent module that backs ``signal.py``.
In CPython this is implemented in ``Modules/signalmodule.c``; in Molt every
entry point is backed by a Rust intrinsic.

Capability gating is NOT performed here -- that is the responsibility of the
higher-level ``signal`` module.  The ``Signals`` IntEnum is likewise defined
only in ``signal.py``.
"""

from __future__ import annotations

from collections.abc import Iterable as _Iterable

from _intrinsics import require_intrinsic as _require_intrinsic

# -- probe intrinsic (standard pattern) ------------------------------------

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
_MOLT_SIGNAL_PTHREAD_SIGMASK = _require_intrinsic("molt_signal_pthread_sigmask")
_MOLT_SIGNAL_PTHREAD_KILL = _require_intrinsic("molt_signal_pthread_kill")
_MOLT_SIGNAL_SIGPENDING = _require_intrinsic("molt_signal_sigpending")
_MOLT_SIGNAL_SIGWAIT = _require_intrinsic("molt_signal_sigwait")

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


# BEGIN PUBLIC OBJECT molt_signal_default_int_handler
# Generated by tools/gen_intrinsics.py from public_objects.toml.
default_int_handler = _require_intrinsic('molt_signal_default_int_handler')
# END PUBLIC OBJECT molt_signal_default_int_handler


def signal(sig: int, handler: object, /) -> object:
    """Set the action for the given signal."""
    return _MOLT_SIGNAL_SIGNAL(int(sig), handler)

# BEGIN PUBLIC OBJECT molt_signal_signal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
signal.__name__ = 'signal'
signal.__qualname__ = 'signal'
signal.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(signal)
# END PUBLIC OBJECT molt_signal_signal


def getsignal(sig: int, /) -> object:
    """Return the current action for the given signal."""
    # The runtime owns SIGINT's `default_int_handler` disposition.
    return _MOLT_SIGNAL_GETSIGNAL(int(sig))

# BEGIN PUBLIC OBJECT molt_signal_getsignal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
getsignal.__name__ = 'getsignal'
getsignal.__qualname__ = 'getsignal'
getsignal.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(getsignal)
# END PUBLIC OBJECT molt_signal_getsignal


def raise_signal(sig: int, /) -> None:
    """Send a signal to the calling process."""
    _MOLT_SIGNAL_RAISE_SIGNAL(int(sig))

# BEGIN PUBLIC OBJECT molt_signal_raise_signal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
raise_signal.__name__ = 'raise_signal'
raise_signal.__qualname__ = 'raise_signal'
raise_signal.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(raise_signal)
# END PUBLIC OBJECT molt_signal_raise_signal


def alarm(seconds: int, /) -> int:
    """Arrange for SIGALRM to arrive after the given number of seconds."""
    return int(_MOLT_SIGNAL_ALARM(int(seconds)))

# BEGIN PUBLIC OBJECT molt_signal_alarm
# Generated by tools/gen_intrinsics.py from public_objects.toml.
alarm.__name__ = 'alarm'
alarm.__qualname__ = 'alarm'
alarm.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(alarm)
# END PUBLIC OBJECT molt_signal_alarm


def pause() -> None:
    """Wait until a signal is received."""
    _MOLT_SIGNAL_PAUSE()

# BEGIN PUBLIC OBJECT molt_signal_pause
# Generated by tools/gen_intrinsics.py from public_objects.toml.
pause.__name__ = 'pause'
pause.__qualname__ = 'pause'
pause.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(pause)
# END PUBLIC OBJECT molt_signal_pause


def set_wakeup_fd(fd: int, /) -> int:
    """Set the file descriptor for the signal wakeup pipe."""
    return int(_MOLT_SIGNAL_SET_WAKEUP_FD(int(fd)))

# BEGIN PUBLIC OBJECT molt_signal_set_wakeup_fd
# Generated by tools/gen_intrinsics.py from public_objects.toml.
set_wakeup_fd.__name__ = 'set_wakeup_fd'
set_wakeup_fd.__qualname__ = 'set_wakeup_fd'
set_wakeup_fd.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(set_wakeup_fd)
# END PUBLIC OBJECT molt_signal_set_wakeup_fd


def valid_signals() -> set[int]:
    """Return the set of valid signal numbers on this platform."""
    result = _MOLT_SIGNAL_VALID_SIGNALS()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)

# BEGIN PUBLIC OBJECT molt_signal_valid_signals
# Generated by tools/gen_intrinsics.py from public_objects.toml.
valid_signals.__name__ = 'valid_signals'
valid_signals.__qualname__ = 'valid_signals'
valid_signals.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(valid_signals)
# END PUBLIC OBJECT molt_signal_valid_signals


def strsignal(signalnum: int, /) -> str | None:
    """Return the system description of the given signal."""
    result = _MOLT_SIGNAL_STRSIGNAL(int(signalnum))
    if result is None:
        return None
    return str(result)

# BEGIN PUBLIC OBJECT molt_signal_strsignal
# Generated by tools/gen_intrinsics.py from public_objects.toml.
strsignal.__name__ = 'strsignal'
strsignal.__qualname__ = 'strsignal'
strsignal.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(strsignal)
# END PUBLIC OBJECT molt_signal_strsignal


def pthread_sigmask(how: int, mask: _Iterable[int], /) -> set[int]:
    """Fetch and/or change the signal mask of the calling thread."""
    result = _MOLT_SIGNAL_PTHREAD_SIGMASK(int(how), list(mask))
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)

# BEGIN PUBLIC OBJECT molt_signal_pthread_sigmask
# Generated by tools/gen_intrinsics.py from public_objects.toml.
pthread_sigmask.__name__ = 'pthread_sigmask'
pthread_sigmask.__qualname__ = 'pthread_sigmask'
pthread_sigmask.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(pthread_sigmask)
# END PUBLIC OBJECT molt_signal_pthread_sigmask


def pthread_kill(thread_id: int, signalnum: int, /) -> None:
    """Send a signal to a thread."""
    _MOLT_SIGNAL_PTHREAD_KILL(int(thread_id), int(signalnum))

# BEGIN PUBLIC OBJECT molt_signal_pthread_kill
# Generated by tools/gen_intrinsics.py from public_objects.toml.
pthread_kill.__name__ = 'pthread_kill'
pthread_kill.__qualname__ = 'pthread_kill'
pthread_kill.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(pthread_kill)
# END PUBLIC OBJECT molt_signal_pthread_kill


def sigpending() -> set[int]:
    """Examine pending signals."""
    result = _MOLT_SIGNAL_SIGPENDING()
    if isinstance(result, (list, tuple)):
        return set(int(s) for s in result)
    return set(result)

# BEGIN PUBLIC OBJECT molt_signal_sigpending
# Generated by tools/gen_intrinsics.py from public_objects.toml.
sigpending.__name__ = 'sigpending'
sigpending.__qualname__ = 'sigpending'
sigpending.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(sigpending)
# END PUBLIC OBJECT molt_signal_sigpending


def sigwait(sigset: _Iterable[int], /) -> int:
    """Wait for a signal."""
    return int(_MOLT_SIGNAL_SIGWAIT(list(sigset)))

# BEGIN PUBLIC OBJECT molt_signal_sigwait
# Generated by tools/gen_intrinsics.py from public_objects.toml.
sigwait.__name__ = 'sigwait'
sigwait.__qualname__ = 'sigwait'
sigwait.__module__ = '_signal'
_require_intrinsic('molt_function_set_builtin')(sigwait)
# END PUBLIC OBJECT molt_signal_sigwait


globals().pop("_require_intrinsic", None)
