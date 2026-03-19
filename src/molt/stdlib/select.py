"""select module shim for Molt."""

from __future__ import annotations

from typing import Any as _Any, Iterable as _Iterable

from _intrinsics import require_intrinsic as _require_intrinsic


error = OSError

_IO_EVENT_READ = 1 << 0
_IO_EVENT_WRITE = 1 << 1
_IO_EVENT_ERROR = 1 << 2

_SELECT_KIND_POLL = 0
_SELECT_KIND_EPOLL = 1
_SELECT_KIND_KQUEUE = 2
_SELECT_KIND_DEVPOLL = 3

_MOLT_SELECT_SELECT = _require_intrinsic("molt_select_select")
_MOLT_SELECT_CONSTANTS = _require_intrinsic("molt_select_constants")
_MOLT_SELECT_POLL = _require_intrinsic("molt_select_poll")
_MOLT_SELECT_EPOLL = _require_intrinsic("molt_select_epoll")
_MOLT_SELECT_DEVPOLL = _require_intrinsic("molt_select_devpoll")
_MOLT_SELECT_FILENO = _require_intrinsic("molt_select_fileno")
_MOLT_SELECTOR_NEW = _require_intrinsic("molt_select_selector_new")
_MOLT_SELECTOR_FILENO = _require_intrinsic("molt_select_selector_fileno")
_MOLT_SELECTOR_LEN = _require_intrinsic("molt_select_selector_len")
_MOLT_SELECTOR_EVENTS = _require_intrinsic("molt_select_selector_events")
_MOLT_SELECTOR_REGISTER = _require_intrinsic("molt_select_selector_register")
_MOLT_SELECTOR_REGISTER_FD = _require_intrinsic(
    "molt_select_selector_register_fd")
_MOLT_SELECTOR_UNREGISTER = _require_intrinsic(
    "molt_select_selector_unregister")
_MOLT_SELECTOR_UNREGISTER_OBJ = _require_intrinsic(
    "molt_select_selector_unregister_obj")
_MOLT_SELECTOR_MODIFY = _require_intrinsic("molt_select_selector_modify")
_MOLT_SELECTOR_MODIFY_OBJ = _require_intrinsic(
    "molt_select_selector_modify_obj")
_MOLT_SELECTOR_POLL = _require_intrinsic("molt_select_selector_poll")
_MOLT_SELECTOR_CLOSE = _require_intrinsic("molt_select_selector_close")
_MOLT_SELECTOR_DROP = _require_intrinsic("molt_select_selector_drop")


def _load_constants() -> dict[str, int]:
    payload = _MOLT_SELECT_CONSTANTS()
    if not isinstance(payload, dict):
        raise RuntimeError("select constants intrinsic returned invalid payload")
    out: dict[str, int] = {}
    for key, value in payload.items():
        out[str(key)] = int(value)
    return out


_SELECT_CONSTANTS = _load_constants()

_HAS_POLL = bool(int(_SELECT_CONSTANTS.get("_HAS_POLL", 0)))
_HAS_EPOLL = bool(int(_SELECT_CONSTANTS.get("_HAS_EPOLL", 0)))
_HAS_KQUEUE = bool(int(_SELECT_CONSTANTS.get("_HAS_KQUEUE", 0)))
_HAS_DEVPOLL = bool(int(_SELECT_CONSTANTS.get("_HAS_DEVPOLL", 0)))


def _const(name: str, default: int) -> int:
    return int(_SELECT_CONSTANTS.get(name, default))


# Internal constants are always available so backend logic stays deterministic.
_POLLIN = _const("POLLIN", 0x001)
_POLLPRI = _const("POLLPRI", 0x002)
_POLLOUT = _const("POLLOUT", 0x004)
_POLLERR = _const("POLLERR", 0x008)
_POLLHUP = _const("POLLHUP", 0x010)
_POLLNVAL = _const("POLLNVAL", 0x020)
_POLLRDNORM = _const("POLLRDNORM", 0x040)
_POLLRDBAND = _const("POLLRDBAND", 0x080)
_POLLWRNORM = _const("POLLWRNORM", _POLLOUT)
_POLLWRBAND = _const("POLLWRBAND", 0x100)

_EPOLLIN = _const("EPOLLIN", _POLLIN)
_EPOLLPRI = _const("EPOLLPRI", _POLLPRI)
_EPOLLOUT = _const("EPOLLOUT", _POLLOUT)
_EPOLLERR = _const("EPOLLERR", _POLLERR)

_KQ_FILTER_READ = _const("KQ_FILTER_READ", -1)
_KQ_FILTER_WRITE = _const("KQ_FILTER_WRITE", -2)
_KQ_EV_DELETE = _const("KQ_EV_DELETE", 0x0002)


_EXPORTED_CONSTANTS: list[str] = []


def _export_constant(name: str) -> None:
    value = _SELECT_CONSTANTS.get(name)
    if value is None:
        return
    globals()[name] = int(value)
    _EXPORTED_CONSTANTS.append(name)


if _HAS_POLL:
    for _name in (
        "POLLIN",
        "POLLPRI",
        "POLLOUT",
        "POLLERR",
        "POLLHUP",
        "POLLNVAL",
        "POLLRDNORM",
        "POLLRDBAND",
        "POLLWRNORM",
        "POLLWRBAND",
        "PIPE_BUF",
    ):
        _export_constant(_name)

if _HAS_EPOLL:
    for _name in (
        "EPOLLIN",
        "EPOLLPRI",
        "EPOLLOUT",
        "EPOLLERR",
        "EPOLLHUP",
        "EPOLLRDHUP",
        "EPOLLET",
        "EPOLLONESHOT",
        "EPOLLEXCLUSIVE",
        "EPOLLWAKEUP",
        "EPOLLMSG",
        "EPOLL_CTL_ADD",
        "EPOLL_CTL_DEL",
        "EPOLL_CTL_MOD",
    ):
        _export_constant(_name)

if _HAS_KQUEUE:
    for _name in (
        "KQ_FILTER_READ",
        "KQ_FILTER_WRITE",
        "KQ_FILTER_AIO",
        "KQ_FILTER_VNODE",
        "KQ_FILTER_PROC",
        "KQ_FILTER_SIGNAL",
        "KQ_FILTER_TIMER",
        "KQ_EV_ADD",
        "KQ_EV_DELETE",
        "KQ_EV_ENABLE",
        "KQ_EV_DISABLE",
        "KQ_EV_CLEAR",
        "KQ_EV_ONESHOT",
        "KQ_EV_EOF",
        "KQ_EV_ERROR",
        "KQ_EV_FLAG1",
        "KQ_EV_SYSFLAGS",
        "KQ_NOTE_DELETE",
        "KQ_NOTE_WRITE",
        "KQ_NOTE_EXTEND",
        "KQ_NOTE_ATTRIB",
        "KQ_NOTE_LINK",
        "KQ_NOTE_RENAME",
        "KQ_NOTE_REVOKE",
        "KQ_NOTE_TRACK",
        "KQ_NOTE_TRACKERR",
        "KQ_NOTE_CHILD",
        "KQ_NOTE_FORK",
        "KQ_NOTE_EXEC",
        "KQ_NOTE_EXIT",
        "KQ_NOTE_PDATAMASK",
        "KQ_NOTE_PCTRLMASK",
        "KQ_NOTE_LOWAT",
    ):
        _export_constant(_name)


def _fileobj_to_fd(fileobj: _Any) -> int:
    return int(_MOLT_SELECT_FILENO(fileobj))


def _to_io_events(eventmask: int) -> int:
    return (
        _IO_EVENT_READ if eventmask & (_POLLIN | _POLLPRI | _EPOLLIN | _EPOLLPRI) else 0
    ) | (_IO_EVENT_WRITE if eventmask & (_POLLOUT | _EPOLLOUT) else 0)


def _io_events_to_poll(eventmask: int) -> int:
    return (
        (_POLLIN if eventmask & _IO_EVENT_READ else 0)
        | (_POLLOUT if eventmask & _IO_EVENT_WRITE else 0)
        | (_POLLERR if eventmask & _IO_EVENT_ERROR else 0)
    )


def _io_events_to_epoll(eventmask: int) -> int:
    return (
        (_EPOLLIN if eventmask & _IO_EVENT_READ else 0)
        | (_EPOLLOUT if eventmask & _IO_EVENT_WRITE else 0)
        | (_EPOLLERR if eventmask & _IO_EVENT_ERROR else 0)
    )


_POLLLIKE_DEFAULT_MASK = _POLLIN | _POLLPRI | _POLLOUT


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

    def _entry_count(self) -> int:
        return int(_MOLT_SELECTOR_LEN(self._require_handle()))

    def _fd_events(self, fd: int) -> int:
        return int(_MOLT_SELECTOR_EVENTS(self._require_handle(), int(fd)))

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


if _HAS_KQUEUE:

    class kevent:
        __slots__ = ("ident", "filter", "flags", "fflags", "data", "udata")

        def __init__(
            self,
            ident: int,
            filter: int = _KQ_FILTER_READ,
            flags: int = 0,
            fflags: int = 0,
            data: int = 0,
            udata: _Any = None,
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

    def _set_fd_events(self, fd: int, events: int) -> None:
        current = self._fd_events(fd)
        if current == events:
            return None
        handle = self._require_handle()
        if current == 0 and events != 0:
            _MOLT_SELECTOR_REGISTER_FD(handle, fd, events)
            return None
        if current != 0 and events == 0:
            _MOLT_SELECTOR_UNREGISTER(handle, fd)
            return None
        _MOLT_SELECTOR_MODIFY(handle, fd, events)


def _poll_like_register(
    self: _PollLikeObject, fileobj: _Any, eventmask: int = _POLLLIKE_DEFAULT_MASK
) -> None:
    io_events = _to_io_events(int(eventmask))
    if io_events == 0:
        raise ValueError(f"Invalid events: {eventmask!r}")
    _MOLT_SELECTOR_REGISTER(self._require_handle(), fileobj, io_events)


def _poll_like_unregister(self: _PollLikeObject, fileobj: _Any) -> None:
    _MOLT_SELECTOR_UNREGISTER_OBJ(self._require_handle(), fileobj)


def _poll_like_modify(self: _PollLikeObject, fileobj: _Any, eventmask: int) -> None:
    io_events = _to_io_events(int(eventmask))
    if io_events == 0:
        raise ValueError(f"Invalid events: {eventmask!r}")
    _MOLT_SELECTOR_MODIFY_OBJ(self._require_handle(), fileobj, io_events)


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
        limit = self._entry_count() or 1
    elif limit <= 0:
        raise ValueError("maxevents must be greater than 0")
    timeout_seconds = _epoll_timeout_to_seconds(float(timeout))
    ready = _MOLT_SELECTOR_POLL(self._require_handle(), timeout_seconds)
    if len(ready) > limit:
        ready = ready[:limit]
    return [(int(fd), _io_events_to_epoll(int(mask))) for fd, mask in ready]


if _HAS_KQUEUE:

    def _kqueue_control(
        self: _KqueueObject,
        changelist: _Iterable[kevent] | None,
        max_events: int,
        timeout: float | None,
    ) -> list[kevent]:
        for change in changelist or ():
            if not isinstance(change, kevent):
                raise TypeError(
                    "changelist must be an iterable of select.kevent objects"
                )
            fd = _fileobj_to_fd(int(change.ident))
            if int(change.filter) == _KQ_FILTER_READ:
                io_flag = _IO_EVENT_READ
            elif int(change.filter) == _KQ_FILTER_WRITE:
                io_flag = _IO_EVENT_WRITE
            else:
                raise OSError(22, "Invalid argument")
            flags = int(change.flags)
            current = self._fd_events(fd)
            if flags & _KQ_EV_DELETE:
                updated = current & ~io_flag
            else:
                updated = current | io_flag
            self._set_fd_events(fd, updated)

        timeout_seconds = _kqueue_timeout_to_seconds(timeout)
        max_events = int(max_events)
        if max_events < 0:
            raise ValueError(
                f"Length of eventlist must be 0 or positive, got {max_events}"
            )
        if max_events == 0:
            return []

        ready = _MOLT_SELECTOR_POLL(self._require_handle(), timeout_seconds)
        out: list[kevent] = []
        for fd, mask in ready:
            fd = int(fd)
            mask = int(mask)
            if mask & _IO_EVENT_READ:
                out.append(kevent(fd, _KQ_FILTER_READ, 0, 0, 0, None))
                if len(out) >= max_events:
                    break
            if mask & _IO_EVENT_WRITE:
                out.append(kevent(fd, _KQ_FILTER_WRITE, 0, 0, 0, None))
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
if _HAS_KQUEUE:
    _KqueueObject.control = _kqueue_control  # type: ignore[method-assign]


# Expose runtime-owned select directly so the API surface reflects a builtin.
select = _MOLT_SELECT_SELECT

if _HAS_POLL:
    poll = _MOLT_SELECT_POLL


if _HAS_EPOLL:
    epoll = _MOLT_SELECT_EPOLL


if _HAS_DEVPOLL:
    devpoll = _MOLT_SELECT_DEVPOLL


if _HAS_KQUEUE:
    kqueue = _KqueueObject


__all__ = [*sorted(_EXPORTED_CONSTANTS), "error", "select"]
if _HAS_POLL:
    __all__.append("poll")
if _HAS_EPOLL:
    __all__.append("epoll")
if _HAS_DEVPOLL:
    __all__.append("devpoll")
if _HAS_KQUEUE:
    __all__.extend(["kqueue", "kevent"])
