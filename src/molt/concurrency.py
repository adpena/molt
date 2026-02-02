"""Stdlib concurrency helpers for Molt channels and tasks."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, TypeVar, cast
import sys

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


_molt_shims = None
_molt_shims_installed = False


def _lookup_intrinsic(name: str) -> Any | None:
    global _molt_shims
    global _molt_shims_installed
    try:
        target = globals()[name]
    except KeyError:
        target = None
    if target is not None:
        return target
    _py_builtins = sys.modules.get("builtins")
    if _py_builtins is None:
        try:
            import builtins as _py_builtins
        except Exception:
            _py_builtins = None
    if _py_builtins is not None:
        fallback = getattr(_py_builtins, name, None)
        if fallback is not None:
            return fallback
    if _molt_shims is None:
        try:
            from molt import shims as _molt_shims  # type: ignore[no-redef]
        except Exception:
            _molt_shims = None
            return None
    if _molt_shims is not None and not _molt_shims_installed:
        install = getattr(_molt_shims, "install", None)
        if callable(install):
            try:
                install()
            except Exception:
                pass
        _molt_shims_installed = True
    fallback = getattr(_molt_shims, name, None)
    if callable(fallback):
        return fallback
    return None


def _call_intrinsic(name: str, *args: Any) -> Any:
    target = _lookup_intrinsic(name)
    if callable(target):
        return target(*args)
    raise RuntimeError(f"{name} intrinsic unavailable")


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is not None:
        return _PENDING_SENTINEL
    pending = _lookup_intrinsic("molt_pending")
    if callable(pending):
        try:
            _PENDING_SENTINEL = pending()
            return _PENDING_SENTINEL
        except Exception:
            pass
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
            await molt_async_sleep(0.0)

    async def recv_async(self) -> T:
        if self._closed:
            raise RuntimeError("Channel is closed")
        while True:
            res = molt_chan_recv(self._handle)
            if not _is_pending(res):
                return cast(T, res)
            await molt_async_sleep(0.0)

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
