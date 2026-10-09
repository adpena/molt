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
from .base_futures import isfuture as isfuture
_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    CancelledError,
    InvalidStateError,
    _EXPOSE_GRAPH,
    _contextvars,
    _require_asyncio_intrinsic,
    _task_registry_current,
    _molt_asyncio_future_cancel_fast,
    _molt_asyncio_future_cancelled,
    _molt_asyncio_future_done,
    _molt_asyncio_future_drop,
    _molt_asyncio_future_exception,
    _molt_asyncio_future_new,
    _molt_asyncio_future_result,
    _molt_asyncio_future_set_exception_fast,
    _molt_asyncio_future_set_result_fast,
    _molt_generic_alias_new,
    _molt_promise_new,
    _molt_promise_set_result,
)

if TYPE_CHECKING:
    from asyncio import Event

GenericAlias = _types.GenericAlias
STACK_DEBUG = 0
base_futures: Any | None = None
events: Any | None = None
exceptions: Any | None = None
format_helpers: Any | None = None

def _get_loop(fut: Any) -> Any:
    try:
        get_loop = fut.get_loop
    except AttributeError:
        return fut._loop
    return get_loop()


class Future:
    _asyncio_future_blocking = False

    @classmethod
    def __class_getitem__(cls, item: Any) -> Any:
        return _require_asyncio_intrinsic(_molt_generic_alias_new, "generic_alias_new")(
            cls, item
        )

    def __init__(self, *, loop: Any | None = None) -> None:
        self._fut_handle: int = _molt_asyncio_future_new()
        self._result: Any = None
        self._exception: BaseException | None = None
        self._cancel_message: Any | None = None
        self._molt_event_owner: Event | None = None
        self._molt_event_token_id: int | None = None
        if _EXPOSE_GRAPH:
            self._asyncio_awaited_by: set["Future"] | None = None
        self._callback_entries: dict[int, tuple[Callable[["Future"], Any], Any | None, bool]] = {}
        self._next_callback_id = 0
        self._loop: Any = _asyncio.get_event_loop() if loop is None else loop

    def cancel(self, msg: Any | None = None) -> bool:
        if self.done():
            return False
        exc = CancelledError() if msg is None else CancelledError(msg)
        self._set_cancelled(exc, msg)
        return True

    def _set_cancelled(self, exc: BaseException, msg: Any | None) -> None:
        self._exception = exc
        self._cancel_message = msg
        _molt_asyncio_future_cancel_fast(self._fut_handle, msg)
        self._invoke_callbacks()

    def cancelled(self) -> bool:
        return bool(_molt_asyncio_future_cancelled(self._fut_handle))

    def done(self) -> bool:
        return bool(_molt_asyncio_future_done(self._fut_handle))

    def result(self) -> Any:
        if not _molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is not set.")
        if _molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        if self._exception is not None:
            raise self._exception
        stored_exc = _molt_asyncio_future_exception(self._fut_handle)
        if stored_exc is not None:
            raise stored_exc
        return _molt_asyncio_future_result(self._fut_handle)

    def exception(self) -> BaseException | None:
        if not _molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Exception is not set.")
        if _molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        return _molt_asyncio_future_exception(self._fut_handle)

    @property
    def _callbacks(self) -> Any:
        if not self._callback_entries:
            return None
        return [(fn, context) for fn, context, _ in self._callback_entries.values()]

    def add_done_callback(
        self, fn: Callable[["Future"], Any], *, context: Any | None = None
    ) -> None:
        self._subscribe_done_callback(fn, context=context)

    def _subscribe_done_callback(
        self, fn: Callable[["Future"], Any], *, context: Any | None = None,
        _wake_waiter: bool = False,
    ) -> int | None:
        if context is None and not _wake_waiter:
            context = _contextvars.copy_context()
        if self.done():
            if _wake_waiter:
                fn(self)
            else:
                self._run_callback(fn, context)
            return None
        key = self._next_callback_id
        self._next_callback_id += 1
        self._callback_entries[key] = (fn, context, _wake_waiter)
        return key

    def _unsubscribe_done_callback(self, key: int | None) -> None:
        if key is not None:
            self._callback_entries.pop(key, None)

    def remove_done_callback(self, fn: Callable[["Future"], Any]) -> int:
        matches = []
        for key, (callback, context, _) in list(self._callback_entries.items()):
            if callback == fn:
                matches.append(key)
        removed = 0
        for key in matches:
            if self._callback_entries.pop(key, None) is not None:
                removed += 1
        return removed

    def get_loop(self) -> Any:
        return self._loop

    def set_result(self, result: Any) -> None:
        if _molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("invalid state")
        self._result = result
        _molt_asyncio_future_set_result_fast(self._fut_handle, result)
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if _molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("invalid state")
        if isinstance(exception, type):
            exception = exception()
        if not isinstance(exception, BaseException):
            raise TypeError("invalid exception object")
        if isinstance(exception, StopIteration):
            original = exception
            exception = RuntimeError(
                "StopIteration interacts badly with generators and cannot be raised into a Future"
            )
            exception.__cause__ = original
            exception.__context__ = original
        self._exception = exception
        _molt_asyncio_future_set_exception_fast(self._fut_handle, exception)
        self._invoke_callbacks()

    def _invoke_callbacks(self) -> None:
        callbacks = self._callback_entries
        self._callback_entries = {}
        try:
            for fn, context, wake_waiter in callbacks.values():
                if wake_waiter:
                    # Queue the waiting task at its subscription position, in
                    # the same FIFO as ordinary callback Handles. Running this
                    # via a Handle would defer that task by an extra turn.
                    fn(self)
                else:
                    self._run_callback(fn, context)
        finally:
            callbacks.clear()

    def _run_callback(self, fn: Callable[["Future"], Any], context: Any | None) -> None:
        self._loop.call_soon(fn, self, context=context)

    def __await__(self) -> Any:
        return _wait_for_future(self).__await__()

    def __repr__(self) -> str:
        if _molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif _molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Future {state}>"

    def __del__(self) -> None:
        handle = getattr(self, "_fut_handle", None)
        if handle is not None:
            _molt_asyncio_future_drop(handle)

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


def _subscribe_completion(
    fut: Any, callback: Any, *, wake_waiter: bool = False
) -> tuple[bool, Any]:
    if (
        isinstance(fut, Future)
        and type(fut).add_done_callback is Future.add_done_callback
        and type(fut).remove_done_callback is Future.remove_done_callback
    ):
        return (True, fut._subscribe_done_callback(callback, _wake_waiter=wake_waiter))
    # Foreign future protocols and subclass overrides own their registration.
    fut.add_done_callback(callback)
    return (False, callback)


def _unsubscribe_completion(fut: Any, subscription: tuple[bool, Any]) -> None:
    private, key = subscription
    if private:
        fut._unsubscribe_done_callback(key)
    else:
        fut.remove_done_callback(key)



async def _wait_for_future(future: Any) -> Any:
    """The compiled Future protocol bridge to the native promise scheduler."""
    if future.done():
        return future.result()
    waiter = _task_registry_current()
    if not isinstance(waiter, Future):
        waiter = None
    if waiter is future:
        raise RuntimeError("Task cannot await on itself")
    if waiter is not None and _get_loop(future) is not waiter.get_loop():
        raise RuntimeError("Task got Future attached to a different loop")
    if waiter is not None:
        waiter._fut_waiter = future
    if _EXPOSE_GRAPH and waiter is not None:
        future_add_to_awaited_by(future, waiter)
    promise = _molt_promise_new()

    def wake(done: Any) -> None:
        _molt_promise_set_result(promise, None)

    subscription = None
    try:
        subscription = _subscribe_completion(future, wake, wake_waiter=True)
        try:
            await promise
        except CancelledError:
            if future.cancelled():
                return future.result()
            raise
        return future.result()
    finally:
        if subscription is not None:
            _unsubscribe_completion(future, subscription)
        if waiter is not None and waiter._fut_waiter is future:
            waiter._fut_waiter = None
        if _EXPOSE_GRAPH and waiter is not None:
            future_discard_from_awaited_by(future, waiter)
