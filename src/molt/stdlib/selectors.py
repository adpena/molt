"""Selectors shim for Molt."""

from __future__ import annotations

from abc import ABCMeta, abstractmethod
from collections import namedtuple
from collections.abc import Mapping
import math
import select
from _intrinsics import require_intrinsic as _require_intrinsic


# Keep selectors in the intrinsic-backed lane: runtime-owned backends are
# provided by select.py and these intrinsics enforce that contract at import.
_MOLT_SELECTOR_NEW = _require_intrinsic("molt_select_selector_new", globals())
_MOLT_SELECTOR_POLL = _require_intrinsic("molt_select_selector_poll", globals())
_MOLT_SELECT_FILENO = _require_intrinsic("molt_select_fileno", globals())
_MOLT_SELECT_DEFAULT_SELECTOR_KIND = _require_intrinsic(
    "molt_select_default_selector_kind", globals()
)
_MOLT_SELECT_BACKEND_AVAILABLE = _require_intrinsic(
    "molt_select_backend_available", globals()
)


EVENT_READ = 1 << 0
EVENT_WRITE = 1 << 1

_SELECT_KIND_POLL = 0
_SELECT_KIND_EPOLL = 1
_SELECT_KIND_KQUEUE = 2
_SELECT_KIND_DEVPOLL = 3


def _backend_available(kind: int) -> bool:
    return bool(_MOLT_SELECT_BACKEND_AVAILABLE(int(kind)))


def _fileobj_to_fd(fileobj):
    return int(_MOLT_SELECT_FILENO(fileobj))


SelectorKey = namedtuple("SelectorKey", ["fileobj", "fd", "events", "data"])


class _SelectorMapping(Mapping):
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


class BaseSelector(metaclass=ABCMeta):
    @abstractmethod
    def register(self, fileobj, events, data=None): ...

    @abstractmethod
    def unregister(self, fileobj): ...

    def modify(self, fileobj, events, data=None):
        self.unregister(fileobj)
        return self.register(fileobj, events, data)

    @abstractmethod
    def select(self, timeout=None): ...

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

    @abstractmethod
    def get_map(self): ...

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


if _backend_available(_SELECT_KIND_POLL):

    class PollSelector(_PollLikeSelector):
        _selector_cls = select.poll
        _EVENT_READ = select.POLLIN
        _EVENT_WRITE = select.POLLOUT


if _backend_available(_SELECT_KIND_EPOLL):

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


if _backend_available(_SELECT_KIND_DEVPOLL):

    class DevpollSelector(_PollLikeSelector):
        _selector_cls = select.devpoll
        _EVENT_READ = select.POLLIN
        _EVENT_WRITE = select.POLLOUT

        def fileno(self):
            return self._selector.fileno()


if _backend_available(_SELECT_KIND_KQUEUE):

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


_default_kind = int(_MOLT_SELECT_DEFAULT_SELECTOR_KIND())

if _default_kind == _SELECT_KIND_KQUEUE and "KqueueSelector" in globals():
    _default_selector_cls = KqueueSelector
elif _default_kind == _SELECT_KIND_EPOLL and "EpollSelector" in globals():
    _default_selector_cls = EpollSelector
elif _default_kind == _SELECT_KIND_DEVPOLL and "DevpollSelector" in globals():
    _default_selector_cls = DevpollSelector
elif _default_kind == _SELECT_KIND_POLL and "PollSelector" in globals():
    _default_selector_cls = PollSelector
else:
    _default_selector_cls = SelectSelector

if _default_selector_cls is SelectSelector:
    for _selector_name in (
        "KqueueSelector",
        "EpollSelector",
        "DevpollSelector",
        "PollSelector",
    ):
        _selector_candidate = globals().get(_selector_name)
        if _selector_candidate is not None:
            _default_selector_cls = _selector_candidate
            break

DefaultSelector = _default_selector_cls
