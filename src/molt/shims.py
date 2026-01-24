"""Dispatch between CPython and runtime Molt shims."""

from __future__ import annotations

import importlib
from typing import Any

from molt import shims_runtime as _runtime

try:
    _IMPL = importlib.import_module("molt.shims_cpython")
except Exception:
    _IMPL = _runtime

_PENDING = getattr(_IMPL, "_PENDING", None)


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
    "molt_task_register_token_owned",
    "molt_spawn",
    "molt_block_on",
    "molt_async_sleep",
    "molt_thread_submit",
    "molt_chan_new",
    "molt_chan_send",
    "molt_chan_recv",
    "molt_chan_drop",
]

if _IMPL is _runtime:
    install = _runtime.install
    load_runtime = _runtime.load_runtime
    stream_new_handle = _runtime.stream_new_handle
    ws_pair_handles = _runtime.ws_pair_handles
    ws_connect_handle = _runtime.ws_connect_handle
    molt_cancel_token_new = _runtime.molt_cancel_token_new
    molt_cancel_token_clone = _runtime.molt_cancel_token_clone
    molt_cancel_token_drop = _runtime.molt_cancel_token_drop
    molt_cancel_token_cancel = _runtime.molt_cancel_token_cancel
    molt_cancel_token_is_cancelled = _runtime.molt_cancel_token_is_cancelled
    molt_cancel_token_set_current = _runtime.molt_cancel_token_set_current
    molt_cancel_token_get_current = _runtime.molt_cancel_token_get_current
    molt_cancelled = _runtime.molt_cancelled
    molt_cancel_current = _runtime.molt_cancel_current
    molt_future_cancel = _runtime.molt_future_cancel
    molt_future_cancel_msg = _runtime.molt_future_cancel_msg
    molt_future_cancel_clear = _runtime.molt_future_cancel_clear
    molt_task_register_token_owned = _runtime.molt_task_register_token_owned
    molt_spawn = _runtime.molt_spawn
    molt_block_on = _runtime.molt_block_on
    molt_async_sleep = _runtime.molt_async_sleep
    molt_thread_submit = _runtime.molt_thread_submit
    molt_chan_new = _runtime.molt_chan_new
    molt_chan_send = _runtime.molt_chan_send
    molt_chan_recv = _runtime.molt_chan_recv
    molt_chan_drop = _runtime.molt_chan_drop
else:
    install = _IMPL.install
    load_runtime = _IMPL.load_runtime
    stream_new_handle = _IMPL.stream_new_handle
    ws_pair_handles = _IMPL.ws_pair_handles
    ws_connect_handle = _IMPL.ws_connect_handle
    molt_cancel_token_new = _IMPL.molt_cancel_token_new
    molt_cancel_token_clone = _IMPL.molt_cancel_token_clone
    molt_cancel_token_drop = _IMPL.molt_cancel_token_drop
    molt_cancel_token_cancel = _IMPL.molt_cancel_token_cancel
    molt_cancel_token_is_cancelled = _IMPL.molt_cancel_token_is_cancelled
    molt_cancel_token_set_current = _IMPL.molt_cancel_token_set_current
    molt_cancel_token_get_current = _IMPL.molt_cancel_token_get_current
    molt_cancelled = _IMPL.molt_cancelled
    molt_cancel_current = _IMPL.molt_cancel_current
    molt_future_cancel = _IMPL.molt_future_cancel
    molt_future_cancel_msg = _IMPL.molt_future_cancel_msg
    molt_future_cancel_clear = _IMPL.molt_future_cancel_clear
    molt_task_register_token_owned = _IMPL.molt_task_register_token_owned
    molt_spawn = _IMPL.molt_spawn
    molt_block_on = _IMPL.molt_block_on
    molt_async_sleep = _IMPL.molt_async_sleep
    molt_thread_submit = _IMPL.molt_thread_submit
    molt_chan_new = _IMPL.molt_chan_new
    molt_chan_send = _IMPL.molt_chan_send
    molt_chan_recv = _IMPL.molt_chan_recv
    molt_chan_drop = _IMPL.molt_chan_drop
