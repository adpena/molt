"""Stdlib concurrency helpers for Molt channels and tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, TypeVar, cast

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
        molt_chan_recv,
        molt_chan_send,
        molt_spawn,
    )

T = TypeVar("T")
_PENDING = 0x7FFD_0000_0000_0000


class Channel:
    def __init__(self, handle: Any, maxsize: int = 0) -> None:
        self._handle = handle
        self._maxsize = maxsize

    def send(self, value: T) -> int:
        while True:
            res = molt_chan_send(self._handle, value)
            if res != _PENDING:
                return res

    def recv(self) -> T:
        while True:
            res = molt_chan_recv(self._handle)
            if res != _PENDING:
                return cast(T, res)

    async def send_async(self, value: T) -> None:
        while True:
            res = molt_chan_send(self._handle, value)
            if res != _PENDING:
                return None
            await molt_async_sleep(0.0)

    async def recv_async(self) -> T:
        while True:
            res = molt_chan_recv(self._handle)
            if res != _PENDING:
                return cast(T, res)
            await molt_async_sleep(0.0)


def channel(maxsize: int = 0) -> Channel[Any]:
    handle = molt_chan_new(maxsize)
    return Channel(handle, maxsize)


def spawn(task: Any) -> None:
    molt_spawn(task)


class CancellationToken:
    def __init__(self) -> None:
        self._token = molt_cancel_token_new(None)
        self._owned = True

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
