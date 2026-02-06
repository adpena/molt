"""Selectors shim for Molt."""

from __future__ import annotations

from abc import ABCMeta, abstractmethod
from collections.abc import Mapping
import molt.concurrency as _molt_concurrency
import sys as _sys
import time as _time

from _intrinsics import require_intrinsic as _require_intrinsic


EVENT_READ = 1 << 0
EVENT_WRITE = 1 << 1

_molt_io_wait_new = _require_intrinsic("molt_io_wait_new")
_molt_block_on = _require_intrinsic("molt_block_on")


def _fileobj_to_fd(fileobj):
    """Return a file descriptor from a file object."""
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


def _fileobj_to_handle(fileobj):
    if isinstance(fileobj, int):
        return fileobj
    if hasattr(fileobj, "fileno"):
        fileno = fileobj.fileno
        try:
            fd = int(fileno() if callable(fileno) else fileno)
        except (AttributeError, TypeError, ValueError):
            raise ValueError(f"Invalid file object: {fileobj!r}") from None
        if fd < 0:
            raise ValueError(f"Invalid file descriptor: {fd}")
        return fd
    if hasattr(fileobj, "_handle"):
        return getattr(fileobj, "_handle")
    raise ValueError("fileobj must be a socket or file descriptor")


class SelectorKey:
    """SelectorKey(fileobj, fd, events, data)."""

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

    def __repr__(self) -> str:
        return (
            "SelectorKey("
            f"fileobj={self.fileobj!r}, fd={self.fd!r}, "
            f"events={self.events!r}, data={self.data!r})"
        )


class _SelectorMapping(Mapping):
    """Mapping of file objects to selector keys."""

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
    """Selector abstract base class."""

    @abstractmethod
    def register(self, fileobj, events, data=None):
        raise NotImplementedError

    @abstractmethod
    def unregister(self, fileobj):
        raise NotImplementedError

    def modify(self, fileobj, events, data=None):
        self.unregister(fileobj)
        return self.register(fileobj, events, data)

    @abstractmethod
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

    @abstractmethod
    def get_map(self):
        raise NotImplementedError

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class _BaseSelectorImpl(BaseSelector):
    """Base selector implementation."""

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


class _MoltSelectorImpl(_BaseSelectorImpl):
    def register(self, fileobj, events, data=None):
        if _molt_io_wait_new is not None:
            _fileobj_to_handle(fileobj)
        return super().register(fileobj, events, data)

    def select(self, timeout=None):
        if not self._fd_to_key:
            if timeout is None:
                return []
            if timeout > 0:
                _time.sleep(timeout)
            return []
        io_wait = _molt_io_wait_new
        block_on = _molt_block_on

        def _deadline_from_timeout(value):
            if value is None:
                return None
            if value <= 0:
                return _time.monotonic()
            return _time.monotonic() + value

        async def _wait_ready():
            deadline = _deadline_from_timeout(timeout)
            chan = _molt_concurrency.channel()
            futures: list[tuple[SelectorKey, object]] = []

            async def _wait_one(key: SelectorKey, fut: object) -> None:
                try:
                    mask = int(await fut)
                except TimeoutError:
                    mask = 0
                try:
                    await chan.send_async((key, mask))
                except Exception:
                    pass

            for key in self._fd_to_key.values():
                handle = _fileobj_to_handle(key.fileobj)
                fut = io_wait(handle, key.events, deadline)
                futures.append((key, fut))
                _molt_concurrency.spawn(_wait_one(key, fut))

            ready: list[tuple[SelectorKey, int]] = []
            if not futures:
                return ready

            key, mask = await chan.recv_async()
            if mask:
                ready.append((key, mask & key.events))

            while True:
                ok, payload = chan.try_recv()
                if not ok:
                    break
                more_key, more_mask = payload
                if more_mask:
                    ready.append((more_key, more_mask & more_key.events))

            for _key, fut in futures:
                cancel = getattr(fut, "cancel", None)
                if callable(cancel):
                    try:
                        cancel()
                    except Exception:
                        pass
            try:
                chan.close()
            except Exception:
                pass
            return ready

        return block_on(_wait_ready())


class SelectSelector(_MoltSelectorImpl):
    """Select-based selector."""


_IS_LINUX = _sys.platform.startswith("linux")
_IS_WIN = _sys.platform == "win32"
_HAS_KQUEUE = not _IS_LINUX and not _IS_WIN
_HAS_POLL = not _IS_WIN

if _HAS_POLL:

    class PollSelector(_MoltSelectorImpl):
        """Poll-based selector."""


if _IS_LINUX:

    class EpollSelector(_MoltSelectorImpl):
        """Epoll-based selector."""


if _HAS_KQUEUE:

    class KqueueSelector(_MoltSelectorImpl):
        """Kqueue-based selector."""


if _HAS_KQUEUE:
    DefaultSelector = KqueueSelector
elif _IS_LINUX:
    DefaultSelector = EpollSelector
elif _HAS_POLL:
    DefaultSelector = PollSelector
else:
    DefaultSelector = SelectSelector
