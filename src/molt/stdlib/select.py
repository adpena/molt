"""select module shim for Molt."""

from __future__ import annotations

import sys
from typing import Any, Iterable

from _intrinsics import require_intrinsic as _require_intrinsic


error = OSError

_IO_EVENT_READ = 1 << 0
_IO_EVENT_WRITE = 1 << 1
_IO_EVENT_ERROR = 1 << 2

_SELECT_KIND_POLL = 0
_SELECT_KIND_EPOLL = 1
_SELECT_KIND_KQUEUE = 2
_SELECT_KIND_DEVPOLL = 3

# poll-style constants
POLLIN = 0x001
POLLPRI = 0x002
POLLOUT = 0x004
POLLERR = 0x008
POLLHUP = 0x010
POLLNVAL = 0x020

# epoll-style constants
EPOLLIN = 0x001
EPOLLPRI = 0x002
EPOLLOUT = 0x004
EPOLLERR = 0x008
EPOLLHUP = 0x010
EPOLLRDHUP = 0x2000
EPOLLET = 1 << 31
EPOLLONESHOT = 1 << 30
EPOLLEXCLUSIVE = 1 << 28
EPOLLWAKEUP = 1 << 29
EPOLLMSG = 0x400
EPOLL_CTL_ADD = 1
EPOLL_CTL_DEL = 2
EPOLL_CTL_MOD = 3

# kqueue-style constants
KQ_FILTER_READ = -1
KQ_FILTER_WRITE = -2
KQ_EV_ADD = 0x0001
KQ_EV_DELETE = 0x0002
KQ_EV_ENABLE = 0x0004
KQ_EV_DISABLE = 0x0008
KQ_EV_EOF = 0x8000
KQ_EV_ERROR = 0x4000

_MOLT_SELECT_SELECT = _require_intrinsic("molt_select_select", globals())
_MOLT_SELECTOR_NEW = _require_intrinsic("molt_select_selector_new", globals())
_MOLT_SELECTOR_FILENO = _require_intrinsic("molt_select_selector_fileno", globals())
_MOLT_SELECTOR_REGISTER = _require_intrinsic("molt_select_selector_register", globals())
_MOLT_SELECTOR_UNREGISTER = _require_intrinsic(
    "molt_select_selector_unregister", globals()
)
_MOLT_SELECTOR_MODIFY = _require_intrinsic("molt_select_selector_modify", globals())
_MOLT_SELECTOR_POLL = _require_intrinsic("molt_select_selector_poll", globals())
_MOLT_SELECTOR_CLOSE = _require_intrinsic("molt_select_selector_close", globals())
_MOLT_SELECTOR_DROP = _require_intrinsic("molt_select_selector_drop", globals())

_HAS_POLL = sys.platform != "win32"
_HAS_EPOLL = sys.platform.startswith("linux")
_HAS_KQUEUE = sys.platform in {
    "darwin",
    "freebsd",
    "openbsd",
    "netbsd",
    "dragonfly",
}
_HAS_DEVPOLL = sys.platform.startswith("sunos")


def _fileobj_to_fd(fileobj: Any) -> int:
    if isinstance(fileobj, int):
        fd = fileobj
    else:
        try:
            fileno = fileobj.fileno
            fd = int(fileno() if callable(fileno) else fileno)
        except (AttributeError, TypeError, ValueError):
            raise ValueError(f"Invalid file object: {fileobj!r}") from None
    if fd < 0:
        raise ValueError(f"Invalid file descriptor: {fd}")
    return fd


def _to_io_events(eventmask: int) -> int:
    return (
        _IO_EVENT_READ if eventmask & (POLLIN | POLLPRI | EPOLLIN | EPOLLPRI) else 0
    ) | (_IO_EVENT_WRITE if eventmask & (POLLOUT | EPOLLOUT) else 0)


def _io_events_to_poll(eventmask: int) -> int:
    return (
        (POLLIN if eventmask & _IO_EVENT_READ else 0)
        | (POLLOUT if eventmask & _IO_EVENT_WRITE else 0)
        | (POLLERR if eventmask & _IO_EVENT_ERROR else 0)
    )


def _io_events_to_epoll(eventmask: int) -> int:
    return (
        (EPOLLIN if eventmask & _IO_EVENT_READ else 0)
        | (EPOLLOUT if eventmask & _IO_EVENT_WRITE else 0)
        | (EPOLLERR if eventmask & _IO_EVENT_ERROR else 0)
    )


_POLLLIKE_DEFAULT_MASK = POLLIN | POLLPRI | POLLOUT


def _poll_timeout_to_seconds(timeout: int | float | None) -> float | None:
    if timeout is None:
        return None
    timeout_ms = int(timeout)
    if timeout_ms < 0:
        return None
    return timeout_ms / 1000.0


def _epoll_timeout_to_seconds(timeout: float) -> float | None:
    value = float(timeout)
    if value < 0.0:
        return None
    return value


def _kqueue_timeout_to_seconds(timeout: float | None) -> float | None:
    if timeout is None:
        return None
    value = float(timeout)
    if value < 0.0:
        raise ValueError("timeout must be positive or None")
    return value


class _SelectorBackend:
    def __init__(self, kind: int) -> None:
        self._handle = _MOLT_SELECTOR_NEW(int(kind))
        self._closed = False

    def _require_handle(self):
        handle = self._handle
        if self._closed or handle is None:
            raise ValueError("I/O operation on closed selector")
        return handle

    def fileno(self) -> int:
        return int(_MOLT_SELECTOR_FILENO(self._require_handle()))

    @property
    def closed(self) -> bool:
        return self._closed

    def _release_handle(self) -> None:
        handle = self._handle
        if self._closed or handle is None:
            return None
        self._closed = True
        self._handle = None
        try:
            _MOLT_SELECTOR_CLOSE(handle)
        finally:
            _MOLT_SELECTOR_DROP(handle)

    def close(self) -> None:
        self._release_handle()

    def __del__(self) -> None:
        try:
            self._release_handle()
        except Exception:
            return None


class _PollLikeObject(_SelectorBackend):
    def __init__(self, kind: int) -> None:
        super().__init__(kind)
        self._fd_to_events: dict[int, int] = {}


class _EpollObject(_PollLikeObject):
    def __init__(self) -> None:
        super().__init__(_SELECT_KIND_EPOLL)


class _DevpollObject(_PollLikeObject):
    def __init__(self) -> None:
        super().__init__(_SELECT_KIND_DEVPOLL)


class _PollObject:
    def __init__(self) -> None:
        self._handle = _MOLT_SELECTOR_NEW(int(_SELECT_KIND_POLL))
        self._closed = False
        self._fd_to_events: dict[int, int] = {}

    def _require_handle(self):
        handle = self._handle
        if self._closed or handle is None:
            raise ValueError("I/O operation on closed selector")
        return handle

    def _release_handle(self) -> None:
        handle = self._handle
        if self._closed or handle is None:
            return None
        self._closed = True
        self._handle = None
        try:
            _MOLT_SELECTOR_CLOSE(handle)
        finally:
            _MOLT_SELECTOR_DROP(handle)

    def __del__(self) -> None:
        try:
            self._release_handle()
        except Exception:
            return None


class kevent:
    __slots__ = ("ident", "filter", "flags", "fflags", "data", "udata")

    def __init__(
        self,
        ident: int,
        filter: int = KQ_FILTER_READ,
        flags: int = 0,
        fflags: int = 0,
        data: int = 0,
        udata: Any = None,
    ) -> None:
        self.ident = int(ident)
        self.filter = int(filter)
        self.flags = int(flags)
        self.fflags = int(fflags)
        self.data = int(data)
        self.udata = udata

    def __repr__(self) -> str:
        return (
            "kevent("
            f"ident={self.ident!r}, filter={self.filter!r}, flags={self.flags!r}, "
            f"fflags={self.fflags!r}, data={self.data!r}, udata={self.udata!r})"
        )


class _KqueueObject(_SelectorBackend):
    def __init__(self) -> None:
        super().__init__(_SELECT_KIND_KQUEUE)
        self._fd_to_events: dict[int, int] = {}

    def _set_fd_events(self, fd: int, events: int) -> None:
        current = self._fd_to_events.get(fd, 0)
        if current == events:
            return None
        handle = self._require_handle()
        if current == 0 and events != 0:
            _MOLT_SELECTOR_REGISTER(handle, fd, events)
            self._fd_to_events[fd] = events
            return None
        if current != 0 and events == 0:
            _MOLT_SELECTOR_UNREGISTER(handle, fd)
            self._fd_to_events.pop(fd, None)
            return None
        _MOLT_SELECTOR_MODIFY(handle, fd, events)
        self._fd_to_events[fd] = events


def _poll_like_register(
    self: _PollLikeObject, fileobj: Any, eventmask: int = _POLLLIKE_DEFAULT_MASK
) -> None:
    fd = _fileobj_to_fd(fileobj)
    io_events = _to_io_events(int(eventmask))
    if io_events == 0:
        raise ValueError(f"Invalid events: {eventmask!r}")
    _MOLT_SELECTOR_REGISTER(self._require_handle(), fileobj, io_events)
    self._fd_to_events[fd] = int(eventmask)


def _poll_like_unregister(self: _PollLikeObject, fileobj: Any) -> None:
    fd = _fileobj_to_fd(fileobj)
    _MOLT_SELECTOR_UNREGISTER(self._require_handle(), fd)
    self._fd_to_events.pop(fd, None)


def _poll_like_modify(self: _PollLikeObject, fileobj: Any, eventmask: int) -> None:
    fd = _fileobj_to_fd(fileobj)
    io_events = _to_io_events(int(eventmask))
    if io_events == 0:
        raise ValueError(f"Invalid events: {eventmask!r}")
    _MOLT_SELECTOR_MODIFY(self._require_handle(), fd, io_events)
    self._fd_to_events[fd] = int(eventmask)


def _poll_like_wait(
    self: _PollLikeObject, timeout: int | float | None = None
) -> list[tuple[int, int]]:
    timeout_seconds = _poll_timeout_to_seconds(timeout)
    ready = _MOLT_SELECTOR_POLL(self._require_handle(), timeout_seconds)
    return [(int(fd), _io_events_to_poll(int(mask))) for fd, mask in ready]


def _epoll_wait(
    self: _EpollObject,
    timeout: float = -1.0,
    maxevents: int = -1,
) -> list[tuple[int, int]]:
    limit = int(maxevents)
    if limit == -1:
        limit = len(self._fd_to_events) or 1
    elif limit <= 0:
        raise ValueError("maxevents must be greater than 0")
    timeout_seconds = _epoll_timeout_to_seconds(float(timeout))
    ready = _MOLT_SELECTOR_POLL(self._require_handle(), timeout_seconds)
    if len(ready) > limit:
        ready = ready[:limit]
    return [(int(fd), _io_events_to_epoll(int(mask))) for fd, mask in ready]


def _kqueue_control(
    self: _KqueueObject,
    changelist: Iterable[kevent] | None,
    max_events: int,
    timeout: float | None,
) -> list[kevent]:
    for change in changelist or ():
        if not isinstance(change, kevent):
            raise TypeError("changelist must be an iterable of select.kevent objects")
        fd = _fileobj_to_fd(int(change.ident))
        if int(change.filter) == KQ_FILTER_READ:
            io_flag = _IO_EVENT_READ
        elif int(change.filter) == KQ_FILTER_WRITE:
            io_flag = _IO_EVENT_WRITE
        else:
            raise OSError(22, "Invalid argument")
        flags = int(change.flags)
        current = self._fd_to_events.get(fd, 0)
        if flags & KQ_EV_DELETE:
            updated = current & ~io_flag
        else:
            updated = current | io_flag
        self._set_fd_events(fd, updated)

    timeout_seconds = _kqueue_timeout_to_seconds(timeout)
    max_events = int(max_events)
    if max_events < 0:
        raise ValueError(f"Length of eventlist must be 0 or positive, got {max_events}")
    if max_events == 0:
        return []

    ready = _MOLT_SELECTOR_POLL(self._require_handle(), timeout_seconds)
    out: list[kevent] = []
    for fd, mask in ready:
        fd = int(fd)
        mask = int(mask)
        if mask & _IO_EVENT_READ:
            out.append(kevent(fd, KQ_FILTER_READ, 0, 0, 0, None))
            if len(out) >= max_events:
                break
        if mask & _IO_EVENT_WRITE:
            out.append(kevent(fd, KQ_FILTER_WRITE, 0, 0, 0, None))
            if len(out) >= max_events:
                break
    return out


_PollLikeObject.register = _poll_like_register  # type: ignore[method-assign]
_PollLikeObject.unregister = _poll_like_unregister  # type: ignore[method-assign]
_PollLikeObject.modify = _poll_like_modify  # type: ignore[method-assign]
_PollLikeObject.poll = _poll_like_wait  # type: ignore[method-assign]
_PollObject.register = _poll_like_register  # type: ignore[method-assign]
_PollObject.unregister = _poll_like_unregister  # type: ignore[method-assign]
_PollObject.modify = _poll_like_modify  # type: ignore[method-assign]
_PollObject.poll = _poll_like_wait  # type: ignore[method-assign]
_EpollObject.poll = _epoll_wait  # type: ignore[method-assign]
_KqueueObject.control = _kqueue_control  # type: ignore[method-assign]


def select(
    rlist: Iterable[Any],
    wlist: Iterable[Any],
    xlist: Iterable[Any],
    timeout: float | None = None,
) -> tuple[list[Any], list[Any], list[Any]]:
    return _MOLT_SELECT_SELECT(rlist, wlist, xlist, timeout)


def poll() -> _PollObject:
    if not _HAS_POLL:
        raise OSError("poll unsupported on this platform")
    return _PollObject()


if _HAS_EPOLL:

    def epoll(sizehint: int = -1, flags: int = 0) -> _EpollObject:
        _ = sizehint
        _ = flags
        return _EpollObject()


if _HAS_DEVPOLL:

    def devpoll() -> _DevpollObject:
        return _DevpollObject()


if _HAS_KQUEUE:

    def kqueue() -> _KqueueObject:
        return _KqueueObject()


__all__ = [
    "EPOLLERR",
    "EPOLLET",
    "EPOLLEXCLUSIVE",
    "EPOLLHUP",
    "EPOLLIN",
    "EPOLLMSG",
    "EPOLLONESHOT",
    "EPOLLOUT",
    "EPOLLPRI",
    "EPOLLRDHUP",
    "EPOLLWAKEUP",
    "EPOLL_CTL_ADD",
    "EPOLL_CTL_DEL",
    "EPOLL_CTL_MOD",
    "KQ_EV_ADD",
    "KQ_EV_DELETE",
    "KQ_EV_DISABLE",
    "KQ_EV_ENABLE",
    "KQ_EV_EOF",
    "KQ_EV_ERROR",
    "KQ_FILTER_READ",
    "KQ_FILTER_WRITE",
    "POLLERR",
    "POLLHUP",
    "POLLIN",
    "POLLNVAL",
    "POLLOUT",
    "POLLPRI",
    "error",
    "poll",
    "select",
]

if _HAS_EPOLL:
    __all__.append("epoll")
if _HAS_DEVPOLL:
    __all__.append("devpoll")
if _HAS_KQUEUE:
    __all__.extend(["kqueue", "kevent"])
