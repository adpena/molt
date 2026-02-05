"""Stdlib concurrency helpers for Molt channels and tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, TypeVar, cast

from molt import intrinsics as _intrinsics

if TYPE_CHECKING:
    from molt._intrinsics import (
        molt_async_sleep,
        molt_cancel_token_cancel,
        molt_cancel_token_clone,
        molt_cancel_token_drop,
        molt_cancel_token_get_current,
        molt_cancel_token_is_cancelled,
        molt_cancel_token_new,
        molt_cancel_token_set_current,
        molt_chan_new,
        molt_chan_drop,
        molt_chan_recv,
        molt_chan_send,
        molt_chan_send_blocking,
        molt_chan_recv_blocking,
        molt_chan_try_recv,
        molt_chan_try_send,
        molt_spawn,
    )

T = TypeVar("T")
_PENDING = 0x7FFD_0000_0000_0000
_PENDING_SENTINEL: Any | None = None


molt_pending = _intrinsics.require("molt_pending", globals())
molt_async_sleep = _intrinsics.require("molt_async_sleep", globals())
molt_chan_new = _intrinsics.require("molt_chan_new", globals())
molt_chan_drop = _intrinsics.require("molt_chan_drop", globals())
molt_chan_recv = _intrinsics.require("molt_chan_recv", globals())
molt_chan_send = _intrinsics.require("molt_chan_send", globals())
molt_chan_send_blocking = _intrinsics.require("molt_chan_send_blocking", globals())
molt_chan_recv_blocking = _intrinsics.require("molt_chan_recv_blocking", globals())
molt_chan_try_recv = _intrinsics.require("molt_chan_try_recv", globals())
molt_chan_try_send = _intrinsics.require("molt_chan_try_send", globals())
molt_spawn = _intrinsics.require("molt_spawn", globals())
molt_cancel_token_new = _intrinsics.require("molt_cancel_token_new", globals())
molt_cancel_token_clone = _intrinsics.require("molt_cancel_token_clone", globals())
molt_cancel_token_drop = _intrinsics.require("molt_cancel_token_drop", globals())
molt_cancel_token_cancel = _intrinsics.require("molt_cancel_token_cancel", globals())
molt_cancel_token_is_cancelled = _intrinsics.require(
    "molt_cancel_token_is_cancelled", globals()
)
molt_cancel_token_set_current = _intrinsics.require(
    "molt_cancel_token_set_current", globals()
)
molt_cancel_token_get_current = _intrinsics.require(
    "molt_cancel_token_get_current", globals()
)


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is not None:
        return _PENDING_SENTINEL
    try:
        _PENDING_SENTINEL = molt_pending()
        return _PENDING_SENTINEL
    except Exception:
        _PENDING_SENTINEL = _PENDING
    return _PENDING_SENTINEL


def _is_pending(value: Any) -> bool:
    sentinel = _pending_sentinel()
    if sentinel is _PENDING:
        return value == _PENDING
    return value is sentinel


class Channel:
    def __init__(self, handle: Any, maxsize: int = 0) -> None:
        self._handle = handle
        self._maxsize = maxsize
        self._closed = False

    def send(self, value: T) -> int:
        if self._closed:
            raise RuntimeError("Channel is closed")
        res = molt_chan_send_blocking(self._handle, value)
        if not _is_pending(res):
            return res
        while True:
            res = molt_chan_send(self._handle, value)
            if not _is_pending(res):
                return res

    def recv(self) -> T:
        if self._closed:
            raise RuntimeError("Channel is closed")
        res = molt_chan_recv_blocking(self._handle)
        if not _is_pending(res):
            return cast(T, res)
        while True:
            res = molt_chan_recv(self._handle)
            if not _is_pending(res):
                return cast(T, res)

    async def send_async(self, value: T) -> None:
        if self._closed:
            raise RuntimeError("Channel is closed")
        while True:
            res = molt_chan_send(self._handle, value)
            if not _is_pending(res):
                return None
            await molt_async_sleep(0.0, None)

    async def recv_async(self) -> T:
        if self._closed:
            raise RuntimeError("Channel is closed")
        while True:
            res = molt_chan_recv(self._handle)
            if not _is_pending(res):
                return cast(T, res)
            await molt_async_sleep(0.0, None)

    def try_send(self, value: T) -> bool:
        if self._closed:
            raise RuntimeError("Channel is closed")
        res = molt_chan_try_send(self._handle, value)
        if _is_pending(res):
            return False
        return True

    def try_recv(self) -> tuple[bool, T | None]:
        if self._closed:
            raise RuntimeError("Channel is closed")
        res = molt_chan_try_recv(self._handle)
        if _is_pending(res):
            return False, None
        return True, cast(T, res)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        molt_chan_drop(self._handle)
        self._handle = None

    def __del__(self) -> None:
        if getattr(self, "_closed", True):
            return
        self.close()


def channel(maxsize: int = 0) -> Channel[Any]:
    handle = molt_chan_new(maxsize)
    return Channel(handle, maxsize)


def spawn(task: Any) -> None:
    molt_spawn(task)


class CancellationToken:
    def __init__(self) -> None:
        self._token = molt_cancel_token_new(None)
        self._owned = True

    @classmethod
    def detached(cls) -> "CancellationToken":
        token = cls()
        old_id = token._token
        token._token = molt_cancel_token_new(-1)
        molt_cancel_token_drop(old_id)
        return token

    def child(self) -> "CancellationToken":
        token = CancellationToken()
        old_id = token._token
        token._token = molt_cancel_token_new(self._token)
        molt_cancel_token_drop(old_id)
        return token

    def cancelled(self) -> bool:
        return molt_cancel_token_is_cancelled(self._token)

    def cancel(self) -> None:
        molt_cancel_token_cancel(self._token)

    def set_current(self) -> "CancellationToken":
        prev_id = molt_cancel_token_set_current(self._token)
        return _wrap_existing_token(prev_id, False)

    def token_id(self) -> int:
        return self._token

    def __del__(self) -> None:
        if getattr(self, "_owned", False):
            molt_cancel_token_drop(self._token)


def _wrap_existing_token(token_id: int, owned: bool) -> CancellationToken:
    token = CancellationToken()
    old_id = token._token
    token._token = token_id
    token._owned = owned
    if owned:
        molt_cancel_token_clone(token_id)
    if old_id != token_id:
        molt_cancel_token_drop(old_id)
    return token


def current_token() -> CancellationToken:
    return _wrap_existing_token(molt_cancel_token_get_current(), False)


def set_current_token(token: CancellationToken) -> CancellationToken:
    prev_id = molt_cancel_token_set_current(token._token)
    return _wrap_existing_token(prev_id, False)


def cancelled() -> bool:
    return molt_cancel_token_is_cancelled(molt_cancel_token_get_current())


def cancel_current() -> None:
    molt_cancel_token_cancel(molt_cancel_token_get_current())
