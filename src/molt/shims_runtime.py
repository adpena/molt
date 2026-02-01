"""Runtime-friendly Molt shims for compiled builds."""

from __future__ import annotations

from typing import Any

from molt.concurrency import (
    CancellationToken,
    cancelled,
    cancel_current,
    channel,
    current_token,
    set_current_token,
    spawn,
)

_PENDING = 0x7FFD_0000_0000_0000
_TOKENS: dict[int, CancellationToken] = {}
_TOKEN_REFS: dict[int, int] = {}


def _track_token(token: CancellationToken) -> int:
    token_id = token.token_id()
    _TOKENS[token_id] = token
    _TOKEN_REFS[token_id] = _TOKEN_REFS.get(token_id, 0) + 1
    return token_id


def molt_cancel_token_new(parent_id: int | None) -> int:
    if parent_id == -1:
        token = CancellationToken.detached()
        return _track_token(token)
    if parent_id in (None, 0):
        token = CancellationToken()
        return _track_token(token)
    parent = _TOKENS.get(parent_id)
    if parent is None:
        token = CancellationToken()
        return _track_token(token)
    token = parent.child()
    return _track_token(token)


def molt_cancel_token_clone(token_id: int) -> int:
    if token_id not in _TOKENS:
        return 0
    _TOKEN_REFS[token_id] = _TOKEN_REFS.get(token_id, 0) + 1
    return token_id


def molt_cancel_token_drop(token_id: int) -> int:
    if token_id not in _TOKEN_REFS:
        return 0
    refs = _TOKEN_REFS[token_id] - 1
    if refs <= 0:
        _TOKEN_REFS.pop(token_id, None)
        _TOKENS.pop(token_id, None)
    else:
        _TOKEN_REFS[token_id] = refs
    return 0


def molt_cancel_token_cancel(token_id: int) -> int:
    token = _TOKENS.get(token_id)
    if token is not None:
        token.cancel()
    return 0


def molt_cancel_token_is_cancelled(token_id: int) -> bool:
    token = _TOKENS.get(token_id)
    if token is None:
        return False
    return token.cancelled()


def molt_cancel_token_set_current(token_id: int | None) -> int:
    token = _TOKENS.get(token_id or 0)
    if token is None:
        token = current_token()
    prev = set_current_token(token)
    return prev.token_id()


def molt_cancel_token_get_current() -> int:
    return current_token().token_id()


def molt_cancelled() -> bool:
    return cancelled()


def molt_cancel_current() -> None:
    cancel_current()


def molt_future_cancel(future: Any) -> int:
    if hasattr(future, "cancel"):
        try:
            future.cancel()
        except Exception:
            return 0
    return 0


def molt_future_cancel_msg(future: Any, msg: Any) -> int:
    if hasattr(future, "cancel"):
        try:
            future.cancel(msg)
        except TypeError:
            try:
                future.cancel()
            except Exception:
                return 0
        except Exception:
            return 0
    return 0


def molt_future_cancel_clear(future: Any) -> int:
    if hasattr(future, "_cancel_message"):
        try:
            setattr(future, "_cancel_message", None)
        except Exception:
            return 0
    return 0


def molt_promise_new() -> Any:
    return None


def molt_promise_set_result(_future: Any, _result: Any) -> int:
    return 0


def molt_promise_set_exception(_future: Any, _exc: Any) -> int:
    return 0


def molt_task_register_token_owned(_task: Any, _token_id: int) -> int:
    return 0


def molt_spawn(task: Any) -> None:
    spawn(task)


def molt_block_on(task: Any) -> Any:
    if callable(task):
        return task()
    return task


async def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
    return _result


def molt_thread_submit(fn: Any, args: Any, kwargs: Any) -> Any:
    async def _run() -> Any:
        if args is None:
            call_args = ()
        else:
            call_args = tuple(args)
        if kwargs is None:
            call_kwargs = {}
        else:
            call_kwargs = dict(kwargs)
        return fn(*call_args, **call_kwargs)

    return _run()


def molt_chan_new(maxsize: int = 0) -> Any:
    return channel(maxsize)


def molt_chan_send(chan: Any, val: Any) -> int:
    if hasattr(chan, "send"):
        return chan.send(val)
    raise TypeError("molt_chan_send expected a Channel")


def molt_chan_try_send(chan: Any, val: Any) -> int:
    if hasattr(chan, "try_send"):
        return 0 if chan.try_send(val) else _PENDING
    return molt_chan_send(chan, val)


def molt_chan_recv(chan: Any) -> Any:
    if hasattr(chan, "recv"):
        return chan.recv()
    raise TypeError("molt_chan_recv expected a Channel")


def molt_chan_try_recv(chan: Any) -> Any:
    if hasattr(chan, "try_recv"):
        ok, value = chan.try_recv()
        return value if ok else _PENDING
    return molt_chan_recv(chan)


def molt_chan_send_blocking(chan: Any, val: Any) -> int:
    if hasattr(chan, "send"):
        return chan.send(val)
    return molt_chan_send(chan, val)


def molt_chan_recv_blocking(chan: Any) -> Any:
    if hasattr(chan, "recv"):
        return chan.recv()
    return molt_chan_recv(chan)


def molt_chan_drop(chan: Any) -> None:
    if hasattr(chan, "close"):
        chan.close()
        return None
    return None


def load_runtime() -> None:
    return None


def stream_new_handle(_lib: Any, _maxsize: int) -> None:
    return None


def ws_pair_handles(_lib: Any, _maxsize: int) -> None:
    return None


def ws_connect_handle(_lib: Any, _url: str) -> None:
    return None


def install() -> None:
    return None
