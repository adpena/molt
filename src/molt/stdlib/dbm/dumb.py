"""Intrinsic-backed ``dbm.dumb`` database for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from typing import Iterator

_MOLT_DBM_DUMB_OPEN = _require_intrinsic("molt_dbm_dumb_open")
_MOLT_DBM_DUMB_GETITEM = _require_intrinsic("molt_dbm_dumb_getitem")
_MOLT_DBM_DUMB_SETITEM = _require_intrinsic("molt_dbm_dumb_setitem")
_MOLT_DBM_DUMB_DELITEM = _require_intrinsic("molt_dbm_dumb_delitem")
_MOLT_DBM_DUMB_CONTAINS = _require_intrinsic("molt_dbm_dumb_contains")
_MOLT_DBM_DUMB_KEYS = _require_intrinsic("molt_dbm_dumb_keys")
_MOLT_DBM_DUMB_SYNC = _require_intrinsic("molt_dbm_dumb_sync")
_MOLT_DBM_DUMB_CLOSE = _require_intrinsic("molt_dbm_dumb_close")

__all__ = ["error", "open"]


class error(OSError):
    pass


class _Database:
    """A simple key-value database backed by Rust intrinsics."""

    def __init__(self, handle: int) -> None:
        self._handle = handle
        self._closed = False

    def __getitem__(self, key: str | bytes) -> bytes:
        if self._closed:
            raise error("DBM object has already been closed")
        return _MOLT_DBM_DUMB_GETITEM(self._handle, key)

    def __setitem__(self, key: str | bytes, value: str | bytes) -> None:
        if self._closed:
            raise error("DBM object has already been closed")
        _MOLT_DBM_DUMB_SETITEM(self._handle, key, value)

    def __delitem__(self, key: str | bytes) -> None:
        if self._closed:
            raise error("DBM object has already been closed")
        _MOLT_DBM_DUMB_DELITEM(self._handle, key)

    def __contains__(self, key: str | bytes) -> bool:
        if self._closed:
            return False
        return _MOLT_DBM_DUMB_CONTAINS(self._handle, key)

    def __iter__(self) -> Iterator[bytes]:
        return iter(self.keys())

    def __len__(self) -> int:
        return len(self.keys())

    def __enter__(self) -> "_Database":
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def __del__(self) -> None:
        if not self._closed:
            self.close()

    def keys(self) -> list[bytes]:
        if self._closed:
            raise error("DBM object has already been closed")
        return _MOLT_DBM_DUMB_KEYS(self._handle)

    def get(self, key: str | bytes, default: bytes | None = None) -> bytes | None:
        try:
            return self[key]
        except KeyError:
            return default

    def sync(self) -> None:
        if self._closed:
            raise error("DBM object has already been closed")
        _MOLT_DBM_DUMB_SYNC(self._handle)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        _MOLT_DBM_DUMB_CLOSE(self._handle)


def open(file: str, flag: str = "c", mode: int = 0o666) -> _Database:
    """Open a dbm.dumb database."""
    handle = _MOLT_DBM_DUMB_OPEN(file, flag, mode)
    return _Database(handle)
