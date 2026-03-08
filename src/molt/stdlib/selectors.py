"""Selectors shim for Molt."""

from __future__ import annotations

from abc import ABCMeta, abstractmethod
from collections import namedtuple
from collections.abc import Mapping
import math
import select
from _intrinsics import require_intrinsic as _require_intrinsic


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
_MOLT_SELECT_FILENO = _require_intrinsic("molt_select_fileno", globals())
_MOLT_SELECT_DEFAULT_SELECTOR_KIND = _require_intrinsic(
    "molt_select_default_selector_kind", globals()
)
_MOLT_SELECT_BACKEND_AVAILABLE = _require_intrinsic(
    "molt_select_backend_available", globals()
)


EVENT_READ = 1 << 0
EVENT_WRITE = 1 << 1

_IO_EVENT_READ = 1 << 0
_IO_EVENT_WRITE = 1 << 1
_IO_EVENT_ERROR = 1 << 2

_SELECT_KIND_POLL = 0
_SELECT_KIND_EPOLL = 1
_SELECT_KIND_KQUEUE = 2
_SELECT_KIND_DEVPOLL = 3


def _backend_available(kind: int) -> bool:
    return bool(_MOLT_SELECT_BACKEND_AVAILABLE(int(kind)))


def _fileobj_to_fd(fileobj):
    return int(_MOLT_SELECT_FILENO(fileobj))


def _normalize_timeout(timeout, *, round_to_ms: bool) -> float | None:
    if timeout is None:
        return None
    value = float(timeout)
    if value <= 0.0:
        return 0.0
    if round_to_ms:
        return math.ceil(value * 1e3) * 1e-3
    return value


def _ready_events(mask: int) -> int:
    events = (EVENT_READ if (mask & _IO_EVENT_READ) else 0) | (
        EVENT_WRITE if (mask & _IO_EVENT_WRITE) else 0
    )
    if mask & _IO_EVENT_ERROR:
        events |= EVENT_READ | EVENT_WRITE
    return events


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


class _IntrinsicSelectorImpl(_BaseSelectorImpl):
    _SELECT_KIND: int
    _ROUND_TIMEOUT_TO_MS = True

    def __init__(self) -> None:
        super().__init__()
        self._selector_handle = _MOLT_SELECTOR_NEW(int(type(self)._SELECT_KIND))
        self._selector_closed = False

    def _require_selector_handle(self):
        handle = self._selector_handle
        if self._selector_closed or handle is None:
            raise ValueError("I/O operation on closed selector")
        return handle

    def register(self, fileobj, events, data=None):
        handle = self._require_selector_handle()
        key = super().register(fileobj, events, data)
        try:
            _MOLT_SELECTOR_REGISTER(handle, fileobj, int(key.events))
        except KeyError:
            self._fd_to_key.pop(key.fd, None)
            raise KeyError(f"{fileobj!r} (FD {key.fd}) is already registered") from None
        except Exception:
            self._fd_to_key.pop(key.fd, None)
            raise
        return key

    def unregister(self, fileobj):
        handle = self._require_selector_handle()
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        try:
            _MOLT_SELECTOR_UNREGISTER(handle, int(key.fd))
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        self._fd_to_key.pop(key.fd, None)
        return key

    def modify(self, fileobj, events, data=None):
        if (not events) or (events & ~(EVENT_READ | EVENT_WRITE)):
            raise ValueError(f"Invalid events: {events!r}")
        handle = self._require_selector_handle()
        try:
            key = self._fd_to_key[self._fileobj_lookup(fileobj)]
        except KeyError:
            raise KeyError(f"{fileobj!r} is not registered") from None
        if events != key.events:
            try:
                _MOLT_SELECTOR_MODIFY(handle, int(key.fd), int(events))
            except KeyError:
                raise KeyError(f"{fileobj!r} is not registered") from None
        if events != key.events or data != key.data:
            key = key._replace(events=events, data=data)
            self._fd_to_key[key.fd] = key
        return key

    def select(self, timeout=None):
        timeout_seconds = _normalize_timeout(
            timeout,
            round_to_ms=self._ROUND_TIMEOUT_TO_MS,
        )
        ready: list[tuple[SelectorKey, int]] = []
        try:
            fd_event_list = _MOLT_SELECTOR_POLL(
                self._require_selector_handle(),
                timeout_seconds,
            )
        except InterruptedError:
            return ready
        fd_to_key_get = self._fd_to_key.get
        for fd, event_mask in fd_event_list:
            key = fd_to_key_get(int(fd))
            if key is None:
                continue
            events = _ready_events(int(event_mask))
            ready.append((key, events & key.events))
        return ready

    def close(self):
        handle = self._selector_handle
        self._selector_closed = True
        self._selector_handle = None
        try:
            if handle is not None:
                try:
                    _MOLT_SELECTOR_CLOSE(handle)
                finally:
                    _MOLT_SELECTOR_DROP(handle)
        finally:
            super().close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            return None


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
        timeout_seconds = _normalize_timeout(timeout, round_to_ms=False)
        ready: list[tuple[SelectorKey, int]] = []
        try:
            readers, writers, _ = select.select(
                list(self._readers),
                list(self._writers),
                [],
                timeout_seconds,
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


if _backend_available(_SELECT_KIND_POLL):

    class PollSelector(_IntrinsicSelectorImpl):
        _SELECT_KIND = _SELECT_KIND_POLL


if _backend_available(_SELECT_KIND_EPOLL):

    class EpollSelector(_IntrinsicSelectorImpl):
        _SELECT_KIND = _SELECT_KIND_EPOLL

        def fileno(self):
            return int(_MOLT_SELECTOR_FILENO(self._require_selector_handle()))

        def select(self, timeout=None):
            ready = super().select(timeout)
            max_events = len(self._fd_to_key) or 1
            if len(ready) > max_events:
                return ready[:max_events]
            return ready


if _backend_available(_SELECT_KIND_DEVPOLL):

    class DevpollSelector(_IntrinsicSelectorImpl):
        _SELECT_KIND = _SELECT_KIND_DEVPOLL

        def fileno(self):
            return int(_MOLT_SELECTOR_FILENO(self._require_selector_handle()))


if _backend_available(_SELECT_KIND_KQUEUE):

    class KqueueSelector(_IntrinsicSelectorImpl):
        _SELECT_KIND = _SELECT_KIND_KQUEUE
        _ROUND_TIMEOUT_TO_MS = False

        def fileno(self):
            return int(_MOLT_SELECTOR_FILENO(self._require_selector_handle()))


_default_kind = int(_MOLT_SELECT_DEFAULT_SELECTOR_KIND())

import sys as _sel_default_sys

_sel_default_mod_dict = _sel_default_sys.modules[__name__].__dict__

if _default_kind == _SELECT_KIND_KQUEUE and "KqueueSelector" in _sel_default_mod_dict:
    _default_selector_cls = KqueueSelector
elif _default_kind == _SELECT_KIND_EPOLL and "EpollSelector" in _sel_default_mod_dict:
    _default_selector_cls = EpollSelector
elif (
    _default_kind == _SELECT_KIND_DEVPOLL and "DevpollSelector" in _sel_default_mod_dict
):
    _default_selector_cls = DevpollSelector
elif _default_kind == _SELECT_KIND_POLL and "PollSelector" in _sel_default_mod_dict:
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
        import sys as _sel_sys

        _sel_mod_dict = (
            getattr(_sel_sys.modules.get(__name__), "__dict__", None) or globals()
        )
        _selector_candidate = _sel_mod_dict.get(_selector_name)
        if _selector_candidate is not None:
            _default_selector_cls = _selector_candidate
            break

DefaultSelector = _default_selector_cls
