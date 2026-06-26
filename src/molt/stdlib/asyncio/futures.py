"""Future authority for ``asyncio.futures``."""

from __future__ import annotations

import asyncio as _asyncio
import concurrent
import contextvars
import logging
import sys
import types as _types
from typing import TYPE_CHECKING, Any, Callable

from _intrinsics import require_intrinsic as _require_intrinsic
from ._debug import (
    _debug_asyncio_exc_enabled,
    _debug_asyncio_promise_enabled,
    _debug_exc_state,
    _debug_tasks_enabled,
    _debug_write,
)

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    CancelledError,
    InvalidStateError,
    _EXPOSE_GRAPH,
    _async_yield_once,
    _contextvars,
    _is_cancelled_exc,
    _molt_future_cancel_msg,
    _require_asyncio_intrinsic,
    _task_registry_current,
    molt_asyncio_future_cancel_fast,
    molt_asyncio_future_cancelled,
    molt_asyncio_future_done,
    molt_asyncio_future_drop,
    molt_asyncio_future_exception,
    molt_asyncio_future_invoke_callbacks,
    molt_asyncio_future_new,
    molt_asyncio_future_result,
    molt_asyncio_future_set_exception_fast,
    molt_asyncio_future_set_result_fast,
    molt_asyncio_running_loop_get,
    molt_future_cancel,
    molt_generic_alias_new,
    molt_promise_new,
    molt_promise_set_exception,
    molt_promise_set_result,
)

if TYPE_CHECKING:
    from asyncio import Event

GenericAlias = _types.GenericAlias
STACK_DEBUG = 0
base_futures: Any | None = None
events: Any | None = None
exceptions: Any | None = None
format_helpers: Any | None = None

def isfuture(obj: Any) -> bool:
    return isinstance(obj, Future)

class Future:
    @classmethod
    def __class_getitem__(cls, item: Any) -> Any:
        return _require_asyncio_intrinsic(molt_generic_alias_new, "generic_alias_new")(
            cls, item
        )

    def __init__(self) -> None:
        self._fut_handle: int = molt_asyncio_future_new()
        self._result: Any = None
        self._exception: BaseException | None = None
        self._cancel_message: Any | None = None
        self._molt_event_owner: Event | None = None
        self._molt_event_token_id: int | None = None
        if _EXPOSE_GRAPH:
            self._asyncio_awaited_by: set["Future"] | None = None
        self._callbacks: list[tuple[Callable[["Future"], Any], Any | None]] = []
        self._molt_promise: Any | None = molt_promise_new()
        self._loop: Any = molt_asyncio_running_loop_get()
        if _debug_asyncio_promise_enabled():
            _debug_write(
                "asyncio_promise_new ok={ok} promise={promise}".format(
                    ok=self._molt_promise is not None,
                    promise=self._molt_promise,
                )
            )

    def cancel(self, msg: Any | None = None) -> bool:
        if molt_asyncio_future_done(self._fut_handle):
            return False
        if _debug_tasks_enabled():
            _debug_write(
                "asyncio_future_cancel type={typ} msg={msg!r}".format(
                    typ=type(self).__name__, msg=msg
                )
            )
        promise = self._molt_promise
        if msg is None:
            _require_asyncio_intrinsic(molt_future_cancel, "future_cancel")(promise)
        else:
            _require_asyncio_intrinsic(_molt_future_cancel_msg, "future_cancel_msg")(
                promise, msg
            )
        molt_asyncio_future_cancel_fast(self._fut_handle, msg)
        self._exception = None
        self._cancel_message = None
        if msg is not None:
            if isinstance(msg, str) or isinstance(msg, bytes):
                self._exception = CancelledError(msg)
            else:
                self._exception = CancelledError()
            self._cancel_message = msg
        self._invoke_callbacks()
        return True

    def cancelled(self) -> bool:
        return bool(molt_asyncio_future_cancelled(self._fut_handle))

    def done(self) -> bool:
        return bool(molt_asyncio_future_done(self._fut_handle))

    def result(self) -> Any:
        if not molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is not set.")
        if molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        if self._exception is not None:
            if _debug_asyncio_exc_enabled():
                exc_name = getattr(type(self._exception), "__name__", "Unknown")
                _debug_write("future_exception_type={name}".format(name=exc_name))
            _debug_exc_state("future_result_before_raise")
            raise self._exception
            _debug_exc_state("future_result_after_raise")
        stored_exc = molt_asyncio_future_exception(self._fut_handle)
        if stored_exc is not None:
            raise stored_exc
        return molt_asyncio_future_result(self._fut_handle)

    def exception(self) -> BaseException | None:
        if not molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Exception is not set.")
        if molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        return molt_asyncio_future_exception(self._fut_handle)

    def add_done_callback(
        self, fn: Callable[["Future"], Any], *, context: Any | None = None
    ) -> None:
        if context is None:
            copy_ctx = getattr(_contextvars, "copy_context", None)
            if callable(copy_ctx):
                context = copy_ctx()
            else:
                context = None
        if molt_asyncio_future_done(self._fut_handle):
            self._run_callback(fn, context)
            return None
        self._callbacks.append((fn, context))
        return None

    def remove_done_callback(self, fn: Callable[["Future"], Any]) -> int:
        filtered = [(f, ctx) for f, ctx in self._callbacks if f is not fn]
        removed = len(self._callbacks) - len(filtered)
        self._callbacks[:] = filtered
        return removed

    def get_loop(self) -> Any:
        return self._loop

    def set_result(self, result: Any) -> None:
        if molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("invalid state")
        self._result = result
        molt_asyncio_future_set_result_fast(self._fut_handle, result)
        if self._molt_promise is not None:
            molt_promise_set_result(self._molt_promise, result)
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("invalid state")
        self._exception = exception
        if _is_cancelled_exc(exception):
            molt_asyncio_future_cancel_fast(self._fut_handle, None)
        else:
            molt_asyncio_future_set_exception_fast(self._fut_handle, exception)
        if self._molt_promise is not None:
            molt_promise_set_exception(self._molt_promise, exception)
        self._invoke_callbacks()

    def _invoke_callbacks(self) -> None:
        callbacks = self._callbacks
        self._callbacks = []
        _require_asyncio_intrinsic(
            molt_asyncio_future_invoke_callbacks, "asyncio_future_invoke_callbacks"
        )(self, callbacks)

    def _run_callback(self, fn: Callable[["Future"], Any], context: Any | None) -> None:
        if context is not None:
            context.run(fn, self)
        else:
            fn(self)

    async def _wait(self) -> Any:
        while not molt_asyncio_future_done(self._fut_handle):
            await _async_yield_once()
        return self.result()

    def __await__(self) -> Any:
        async def _wrapped() -> Any:
            waiter = None
            if _EXPOSE_GRAPH:
                waiter = _task_registry_current()
                if isinstance(waiter, Future):
                    future_add_to_awaited_by(self, waiter)
            try:
                if _debug_asyncio_promise_enabled():
                    _debug_write("asyncio_promise_await")
                return await self._molt_promise
            finally:
                if _EXPOSE_GRAPH and isinstance(waiter, Future):
                    future_discard_from_awaited_by(self, waiter)

        return _wrapped().__await__()

    def __repr__(self) -> str:
        if molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Future {state}>"

    def __del__(self) -> None:
        handle = getattr(self, "_fut_handle", None)
        if handle is not None:
            molt_asyncio_future_drop(handle)

def future_add_to_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is None:
            fut._asyncio_awaited_by = set()
        fut._asyncio_awaited_by.add(waiter)

def future_discard_from_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is not None:
            fut._asyncio_awaited_by.discard(waiter)


def wrap_future(fut: Any, *, loop: Any | None = None) -> Future:
    return _asyncio.wrap_future(fut, loop=loop)

__all__ = [
    "Future",
    "GenericAlias",
    "STACK_DEBUG",
    "base_futures",
    "concurrent",
    "contextvars",
    "events",
    "exceptions",
    "format_helpers",
    "isfuture",
    "logging",
    "sys",
    "wrap_future",
]
if _EXPOSE_GRAPH:
    __all__.extend(["future_add_to_awaited_by", "future_discard_from_awaited_by"])

globals().pop("_require_intrinsic", None)
