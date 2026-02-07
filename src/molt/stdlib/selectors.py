"""Selectors shim for Molt."""

from __future__ import annotations

import math
import select
from _intrinsics import require_intrinsic as _require_intrinsic


# Keep selectors in the intrinsic-backed lane: runtime-owned backends are
# provided by select.py and these intrinsics enforce that contract at import.
_MOLT_SELECTOR_NEW = _require_intrinsic("molt_select_selector_new", globals())
_MOLT_SELECTOR_POLL = _require_intrinsic("molt_select_selector_poll", globals())


EVENT_READ = 1 << 0
EVENT_WRITE = 1 << 1


def _fileobj_to_fd(fileobj):
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


class SelectorKey:
    __slots__ = ("fileobj", "fd", "events", "data")

    def __init__(self, fileobj, fd, events, data) -> None:
        self.fileobj = fileobj
        self.fd = fd
        self.events = events
        self.data = data

    def _replace(self, **kwargs):
        return SelectorKey(
            kwargs.get("fileobj", self.fileobj),
            kwargs.get("fd", self.fd),
            kwargs.get("events", self.events),
            kwargs.get("data", self.data),
        )

    def __iter__(self):
        return iter((self.fileobj, self.fd, self.events, self.data))

    def __len__(self) -> int:
        return 4

    def __getitem__(self, idx):
        return (self.fileobj, self.fd, self.events, self.data)[idx]


class _SelectorMapping:
    def __init__(self, selector: "_BaseSelectorImpl") -> None:
        self._selector = selector

    def __len__(self) -> int:
        return len(self._selector._fd_to_key)

    def get(self, fileobj, default=None):
        fd = self._selector._fileobj_lookup(fileobj)
        return self._selector._fd_to_key.get(fd, default)

    def __getitem__(self, fileobj):
        fd = self._selector._fileobj_lookup(fileobj)
        key = self._selector._fd_to_key.get(fd)
        if key is None:
            raise KeyError(f"{fileobj!r} is not registered")
        return key

    def __iter__(self):
        return iter(self._selector._fd_to_key)


class BaseSelector:
    def register(self, fileobj, events, data=None):
        raise NotImplementedError

    def unregister(self, fileobj):
        raise NotImplementedError

    def modify(self, fileobj, events, data=None):
        self.unregister(fileobj)
        return self.register(fileobj, events, data)

    def select(self, timeout=None):
        raise NotImplementedError

    def close(self) -> None:
        pass

    def get_key(self, fileobj):
        mapping = self.get_map()
        if mapping is None:
            raise RuntimeError("Selector is closed")
        try:
            return mapping[fileobj]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None

    def get_map(self):
        raise NotImplementedError

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class _BaseSelectorImpl(BaseSelector):
    def __init__(self) -> None:
        self._fd_to_key: dict[int, SelectorKey] = {}
        self._map: _SelectorMapping | None = _SelectorMapping(self)

    def _fileobj_lookup(self, fileobj):
        try:
            return _fileobj_to_fd(fileobj)
        except ValueError:
            for key in self._fd_to_key.values():
                if key.fileobj is fileobj:
                    return key.fd
            raise

    def register(self, fileobj, events, data=None):
        if (not events) or (events & ~(EVENT_READ | EVENT_WRITE)):
            raise ValueError(f"Invalid events: {events!r}")
        key = SelectorKey(fileobj, self._fileobj_lookup(fileobj), events, data)
        if key.fd in self._fd_to_key:
            raise KeyError(f"{fileobj!r} (FD {key.fd}) is already registered")
        self._fd_to_key[key.fd] = key
        return key

    def unregister(self, fileobj):
        try:
            key = self._fd_to_key.pop(self._fileobj_lookup(fileobj))
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        return key

    def modify(self, fileobj, events, data=None):
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        if events != key.events:
            self.unregister(fileobj)
            key = self.register(fileobj, events, data)
        elif data != key.data:
            key = key._replace(data=data)
            self._fd_to_key[key.fd] = key
        return key

    def close(self) -> None:
        self._fd_to_key.clear()
        self._map = None

    def get_map(self):
        return self._map


class SelectSelector(_BaseSelectorImpl):
    def __init__(self) -> None:
        super().__init__()
        self._readers: set[int] = set()
        self._writers: set[int] = set()

    def register(self, fileobj, events, data=None):
        key = super().register(fileobj, events, data)
        if events & EVENT_READ:
            self._readers.add(key.fd)
        if events & EVENT_WRITE:
            self._writers.add(key.fd)
        return key

    def unregister(self, fileobj):
        key = super().unregister(fileobj)
        self._readers.discard(key.fd)
        self._writers.discard(key.fd)
        return key

    def select(self, timeout=None):
        timeout = None if timeout is None else max(float(timeout), 0.0)
        ready: list[tuple[SelectorKey, int]] = []
        try:
            readers, writers, _ = select.select(
                list(self._readers),
                list(self._writers),
                [],
                timeout,
            )
        except InterruptedError:
            return ready
        reader_set = frozenset(readers)
        writer_set = frozenset(writers)
        fd_to_key_get = self._fd_to_key.get
        for fd in reader_set | writer_set:
            key = fd_to_key_get(fd)
            if key:
                events = (EVENT_READ if fd in reader_set else 0) | (
                    EVENT_WRITE if fd in writer_set else 0
                )
                ready.append((key, events & key.events))
        return ready


class _PollLikeSelector(_BaseSelectorImpl):
    _selector_cls = None
    _EVENT_READ = None
    _EVENT_WRITE = None

    def __init__(self):
        super().__init__()
        # Access via the class to avoid descriptor binding when the backend
        # constructor is a Python function object (e.g. select.poll shim).
        selector_cls = type(self)._selector_cls
        if selector_cls is None:
            raise RuntimeError("poll-like selector backend unavailable")
        self._selector = selector_cls()

    def register(self, fileobj, events, data=None):
        key = super().register(fileobj, events, data)
        backend_events = (self._EVENT_READ if (events & EVENT_READ) else 0) | (
            self._EVENT_WRITE if (events & EVENT_WRITE) else 0
        )
        try:
            self._selector.register(key.fd, backend_events)
        except Exception:
            super().unregister(fileobj)
            raise
        return key

    def unregister(self, fileobj):
        key = super().unregister(fileobj)
        try:
            self._selector.unregister(key.fd)
        except OSError:
            pass
        return key

    def modify(self, fileobj, events, data=None):
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        changed = False
        if events != key.events:
            backend_events = (self._EVENT_READ if (events & EVENT_READ) else 0) | (
                self._EVENT_WRITE if (events & EVENT_WRITE) else 0
            )
            self._selector.modify(key.fd, backend_events)
            changed = True
        if data != key.data:
            changed = True
        if changed:
            key = key._replace(events=events, data=data)
            self._fd_to_key[key.fd] = key
        return key

    def select(self, timeout=None):
        if timeout is None:
            poll_timeout = None
        elif timeout <= 0:
            poll_timeout = 0
        else:
            poll_timeout = math.ceil(float(timeout) * 1e3)
        ready: list[tuple[SelectorKey, int]] = []
        try:
            fd_event_list = self._selector.poll(poll_timeout)
        except InterruptedError:
            return ready
        fd_to_key_get = self._fd_to_key.get
        for fd, event in fd_event_list:
            key = fd_to_key_get(fd)
            if key:
                events = (EVENT_READ if (event & self._EVENT_READ) else 0) | (
                    EVENT_WRITE if (event & self._EVENT_WRITE) else 0
                )
                ready.append((key, events & key.events))
        return ready

    def close(self):
        close_fn = getattr(self._selector, "close", None)
        if close_fn is not None:
            close_fn()
        super().close()


if hasattr(select, "poll"):

    class PollSelector(_PollLikeSelector):
        _selector_cls = select.poll
        _EVENT_READ = select.POLLIN
        _EVENT_WRITE = select.POLLOUT


if hasattr(select, "epoll"):

    class EpollSelector(_PollLikeSelector):
        _selector_cls = select.epoll
        _EVENT_READ = select.EPOLLIN
        _EVENT_WRITE = select.EPOLLOUT

        def fileno(self):
            return self._selector.fileno()

        def select(self, timeout=None):
            if timeout is None:
                epoll_timeout = -1.0
            elif timeout <= 0:
                epoll_timeout = 0.0
            else:
                epoll_timeout = math.ceil(float(timeout) * 1e3) * 1e-3
            max_events = len(self._fd_to_key) or 1
            ready: list[tuple[SelectorKey, int]] = []
            try:
                fd_event_list = self._selector.poll(epoll_timeout)
            except InterruptedError:
                return ready
            if len(fd_event_list) > max_events:
                fd_event_list = fd_event_list[:max_events]
            fd_to_key_get = self._fd_to_key.get
            for fd, event in fd_event_list:
                key = fd_to_key_get(fd)
                if key:
                    events = (EVENT_READ if (event & self._EVENT_READ) else 0) | (
                        EVENT_WRITE if (event & self._EVENT_WRITE) else 0
                    )
                    ready.append((key, events & key.events))
            return ready


if hasattr(select, "devpoll"):

    class DevpollSelector(_PollLikeSelector):
        _selector_cls = select.devpoll
        _EVENT_READ = select.POLLIN
        _EVENT_WRITE = select.POLLOUT

        def fileno(self):
            return self._selector.fileno()


if hasattr(select, "kqueue") and hasattr(select, "kevent"):

    class KqueueSelector(_BaseSelectorImpl):
        def __init__(self):
            super().__init__()
            self._selector = select.kqueue()
            self._max_events = 0

        def fileno(self):
            return self._selector.fileno()

        def register(self, fileobj, events, data=None):
            key = super().register(fileobj, events, data)
            try:
                if events & EVENT_READ:
                    kev = select.kevent(
                        key.fd,
                        select.KQ_FILTER_READ,
                        select.KQ_EV_ADD,
                    )
                    self._selector.control([kev], 0, 0)
                    self._max_events += 1
                if events & EVENT_WRITE:
                    kev = select.kevent(
                        key.fd,
                        select.KQ_FILTER_WRITE,
                        select.KQ_EV_ADD,
                    )
                    self._selector.control([kev], 0, 0)
                    self._max_events += 1
            except Exception:
                super().unregister(fileobj)
                raise
            return key

        def unregister(self, fileobj):
            key = super().unregister(fileobj)
            if key.events & EVENT_READ:
                kev = select.kevent(
                    key.fd,
                    select.KQ_FILTER_READ,
                    select.KQ_EV_DELETE,
                )
                self._max_events -= 1
                try:
                    self._selector.control([kev], 0, 0)
                except OSError:
                    pass
            if key.events & EVENT_WRITE:
                kev = select.kevent(
                    key.fd,
                    select.KQ_FILTER_WRITE,
                    select.KQ_EV_DELETE,
                )
                self._max_events -= 1
                try:
                    self._selector.control([kev], 0, 0)
                except OSError:
                    pass
            return key

        def select(self, timeout=None):
            timeout = None if timeout is None else max(float(timeout), 0.0)
            max_events = self._max_events or 1
            ready: list[tuple[SelectorKey, int]] = []
            try:
                kev_list = self._selector.control(None, max_events, timeout)
            except InterruptedError:
                return ready
            fd_to_key_get = self._fd_to_key.get
            for kev in kev_list:
                fd = int(kev.ident)
                key = fd_to_key_get(fd)
                if not key:
                    continue
                events = (
                    EVENT_READ if int(kev.filter) == select.KQ_FILTER_READ else 0
                ) | (EVENT_WRITE if int(kev.filter) == select.KQ_FILTER_WRITE else 0)
                ready.append((key, events & key.events))
            return ready

        def close(self):
            self._selector.close()
            super().close()


def _can_use(method: str) -> bool:
    selector_ctor = getattr(select, method, None)
    if selector_ctor is None:
        return False
    try:
        selector_obj = selector_ctor()
        if method == "poll":
            selector_obj.poll(0)
        else:
            selector_obj.close()
        return True
    except OSError:
        return False


if _can_use("kqueue"):
    DefaultSelector = KqueueSelector
elif _can_use("epoll"):
    DefaultSelector = EpollSelector
elif _can_use("devpoll"):
    DefaultSelector = DevpollSelector
elif _can_use("poll"):
    DefaultSelector = PollSelector
else:
    DefaultSelector = SelectSelector
