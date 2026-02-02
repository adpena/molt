"""Dispatch between CPython and runtime Molt shims."""

from __future__ import annotations

from typing import Any

from molt import shims_runtime as _runtime

_IMPL = _runtime
_PENDING = getattr(_runtime, "_PENDING", None)


def _load_cpython_shims() -> Any | None:
    try:
        import importlib

        return importlib.import_module("molt.shims_cpython")
    except Exception:
        return None


def _ensure_cpython_shims() -> None:
    global _IMPL, _PENDING
    if _IMPL is not _runtime:
        return
    cpython_impl = _load_cpython_shims()
    if cpython_impl is None:
        return
    _IMPL = cpython_impl
    _PENDING = getattr(_IMPL, "_PENDING", _PENDING)


def __getattr__(name: str) -> Any:
    return getattr(_IMPL, name)


__all__ = [
    "_PENDING",
    "install",
    "load_runtime",
    "stream_new_handle",
    "ws_pair_handles",
    "ws_connect_handle",
    "molt_cancel_token_new",
    "molt_cancel_token_clone",
    "molt_cancel_token_drop",
    "molt_cancel_token_cancel",
    "molt_cancel_token_is_cancelled",
    "molt_cancel_token_set_current",
    "molt_cancel_token_get_current",
    "molt_cancelled",
    "molt_cancel_current",
    "molt_future_cancel",
    "molt_future_cancel_msg",
    "molt_future_cancel_clear",
    "molt_promise_new",
    "molt_promise_set_result",
    "molt_promise_set_exception",
    "molt_task_register_token_owned",
    "molt_spawn",
    "molt_block_on",
    "molt_async_sleep",
    "molt_thread_submit",
    "molt_chan_new",
    "molt_chan_send",
    "molt_chan_recv",
    "molt_chan_try_send",
    "molt_chan_try_recv",
    "molt_chan_send_blocking",
    "molt_chan_recv_blocking",
    "molt_chan_drop",
]


def install() -> None:
    _ensure_cpython_shims()
    install_impl = getattr(_IMPL, "install", None)
    if callable(install_impl):
        install_impl()


def load_runtime() -> Any:
    _ensure_cpython_shims()
    loader = getattr(_IMPL, "load_runtime", None)
    if callable(loader):
        return loader()
    return None


def stream_new_handle(*args: Any, **kwargs: Any) -> Any:
    _ensure_cpython_shims()
    return _IMPL.stream_new_handle(*args, **kwargs)


def ws_pair_handles(*args: Any, **kwargs: Any) -> Any:
    _ensure_cpython_shims()
    return _IMPL.ws_pair_handles(*args, **kwargs)


def ws_connect_handle(*args: Any, **kwargs: Any) -> Any:
    _ensure_cpython_shims()
    return _IMPL.ws_connect_handle(*args, **kwargs)


def _passthrough(name: str, *args: Any, **kwargs: Any) -> Any:
    _ensure_cpython_shims()
    target = getattr(_IMPL, name)
    return target(*args, **kwargs)


def molt_cancel_token_new(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_new", *args, **kwargs)


def molt_cancel_token_clone(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_clone", *args, **kwargs)


def molt_cancel_token_drop(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_drop", *args, **kwargs)


def molt_cancel_token_cancel(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_cancel", *args, **kwargs)


def molt_cancel_token_is_cancelled(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_is_cancelled", *args, **kwargs)


def molt_cancel_token_set_current(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_set_current", *args, **kwargs)


def molt_cancel_token_get_current(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_token_get_current", *args, **kwargs)


def molt_cancelled(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancelled", *args, **kwargs)


def molt_cancel_current(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_cancel_current", *args, **kwargs)


def molt_future_cancel(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_future_cancel", *args, **kwargs)


def molt_future_cancel_msg(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_future_cancel_msg", *args, **kwargs)


def molt_future_cancel_clear(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_future_cancel_clear", *args, **kwargs)


def molt_promise_new(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_promise_new", *args, **kwargs)


def molt_promise_set_result(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_promise_set_result", *args, **kwargs)


def molt_promise_set_exception(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_promise_set_exception", *args, **kwargs)


def molt_task_register_token_owned(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_task_register_token_owned", *args, **kwargs)


def molt_spawn(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_spawn", *args, **kwargs)


def molt_block_on(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_block_on", *args, **kwargs)


def molt_async_sleep(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_async_sleep", *args, **kwargs)


def molt_thread_submit(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_thread_submit", *args, **kwargs)


def molt_chan_new(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_new", *args, **kwargs)


def molt_chan_send(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_send", *args, **kwargs)


def molt_chan_recv(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_recv", *args, **kwargs)


def molt_chan_try_send(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_try_send", *args, **kwargs)


def molt_chan_try_recv(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_try_recv", *args, **kwargs)


def molt_chan_send_blocking(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_send_blocking", *args, **kwargs)


def molt_chan_recv_blocking(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_recv_blocking", *args, **kwargs)


def molt_chan_drop(*args: Any, **kwargs: Any) -> Any:
    return _passthrough("molt_chan_drop", *args, **kwargs)
