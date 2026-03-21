"""fcntl — file descriptor control for non-blocking I/O.

Provides fcntl(fd, cmd[, arg]) and the associated constants needed by trio
and other async I/O libraries to set sockets to non-blocking mode.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")

# Intrinsic bindings
_MOLT_FCNTL = _require_intrinsic("molt_fcntl")
_MOLT_FCNTL_F_GETFL = _require_intrinsic("molt_fcntl_f_getfl")
_MOLT_FCNTL_F_SETFL = _require_intrinsic("molt_fcntl_f_setfl")
_MOLT_FCNTL_F_GETFD = _require_intrinsic("molt_fcntl_f_getfd")
_MOLT_FCNTL_F_SETFD = _require_intrinsic("molt_fcntl_f_setfd")
_MOLT_FCNTL_FD_CLOEXEC = _require_intrinsic("molt_fcntl_fd_cloexec")
_MOLT_FCNTL_O_NONBLOCK = _require_intrinsic("molt_fcntl_o_nonblock")

# Constants — resolved at import time from the runtime so they match the
# host platform (Linux vs macOS vs WASM).
F_GETFL: int = int(_MOLT_FCNTL_F_GETFL())
F_SETFL: int = int(_MOLT_FCNTL_F_SETFL())
F_GETFD: int = int(_MOLT_FCNTL_F_GETFD())
F_SETFD: int = int(_MOLT_FCNTL_F_SETFD())
FD_CLOEXEC: int = int(_MOLT_FCNTL_FD_CLOEXEC())
O_NONBLOCK: int = int(_MOLT_FCNTL_O_NONBLOCK())

# Additional constants that CPython's fcntl exposes (commonly used).
F_DUPFD = 0
F_DUPFD_CLOEXEC = 1030  # Linux value; macOS uses 67
LOCK_SH = 1
LOCK_EX = 2
LOCK_NB = 4
LOCK_UN = 8


def fcntl(fd, cmd, arg=0):
    """Perform the operation *cmd* on file descriptor *fd*.

    The values *cmd* and *arg* are integers; see the C library ``fcntl``
    man page for details.  When *arg* is omitted it defaults to 0.

    This is the primary entry point used by trio:
        flags = fcntl.fcntl(fd, F_GETFL)
        fcntl.fcntl(fd, F_SETFL, flags | O_NONBLOCK)
    """
    if hasattr(fd, "fileno"):
        fd = fd.fileno()
    return int(_MOLT_FCNTL(fd, cmd, arg))


def ioctl(fd, request, arg=0, mutate_flag=True):
    """ioctl stub — raises OSError for unsupported requests."""
    raise OSError("ioctl is not supported in this runtime")


def flock(fd, operation):
    """flock stub — raises OSError."""
    raise OSError("flock is not supported in this runtime")


def lockf(fd, cmd, len=0, start=0, whence=0):
    """lockf stub — raises OSError."""
    raise OSError("lockf is not supported in this runtime")
