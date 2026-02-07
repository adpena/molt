"""Intrinsic-backed context manager helpers for Molt."""

from __future__ import annotations

from typing import Any, Callable

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): add remaining contextlib parity (`AbstractContextManager` and full edge-case semantics).

__all__ = [
    "ContextDecorator",
    "AsyncExitStack",
    "ExitStack",
    "aclosing",
    "contextmanager",
    "asynccontextmanager",
    "closing",
    "nullcontext",
    "redirect_stderr",
    "redirect_stdout",
    "suppress",
]


_MOLT_CONTEXT_NULL = _require_intrinsic("molt_context_null", globals())
_MOLT_CONTEXTLIB_CLOSING = _require_intrinsic("molt_contextlib_closing", globals())
_MOLT_CONTEXTLIB_ACLOSING_ENTER = _require_intrinsic(
    "molt_contextlib_aclosing_enter", globals()
)
_MOLT_CONTEXTLIB_ACLOSING_EXIT = _require_intrinsic(
    "molt_contextlib_aclosing_exit", globals()
)
_MOLT_CONTEXTLIB_ASYNCGEN_CM_NEW = _require_intrinsic(
    "molt_contextlib_asyncgen_cm_new", globals()
)
_MOLT_CONTEXTLIB_ASYNCGEN_CM_DROP = _require_intrinsic(
    "molt_contextlib_asyncgen_cm_drop", globals()
)
_MOLT_CONTEXTLIB_ASYNCGEN_CM_AENTER = _require_intrinsic(
    "molt_contextlib_asyncgen_cm_aenter", globals()
)
_MOLT_CONTEXTLIB_ASYNCGEN_CM_AEXIT = _require_intrinsic(
    "molt_contextlib_asyncgen_cm_aexit", globals()
)
_MOLT_CONTEXTLIB_GENERATOR_ENTER = _require_intrinsic(
    "molt_contextlib_generator_enter", globals()
)
_MOLT_CONTEXTLIB_GENERATOR_EXIT = _require_intrinsic(
    "molt_contextlib_generator_exit", globals()
)
_MOLT_CONTEXTLIB_SUPPRESS_MATCH = _require_intrinsic(
    "molt_contextlib_suppress_match", globals()
)
_MOLT_CONTEXTLIB_REDIRECT_ENTER = _require_intrinsic(
    "molt_contextlib_redirect_enter", globals()
)
_MOLT_CONTEXTLIB_REDIRECT_EXIT = _require_intrinsic(
    "molt_contextlib_redirect_exit", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_NEW = _require_intrinsic(
    "molt_contextlib_exitstack_new", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_DROP = _require_intrinsic(
    "molt_contextlib_exitstack_drop", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_PUSH = _require_intrinsic(
    "molt_contextlib_exitstack_push", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_PUSH_CALLBACK = _require_intrinsic(
    "molt_contextlib_exitstack_push_callback", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_POP = _require_intrinsic(
    "molt_contextlib_exitstack_pop", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_POP_ALL = _require_intrinsic(
    "molt_contextlib_exitstack_pop_all", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_EXIT = _require_intrinsic(
    "molt_contextlib_exitstack_exit", globals()
)
_MOLT_CONTEXTLIB_EXITSTACK_ENTER_CONTEXT = _require_intrinsic(
    "molt_contextlib_exitstack_enter_context", globals()
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_CALLBACK = _require_intrinsic(
    "molt_contextlib_async_exitstack_push_callback", globals()
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_EXIT = _require_intrinsic(
    "molt_contextlib_async_exitstack_push_exit", globals()
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT = _require_intrinsic(
    "molt_contextlib_async_exitstack_enter_context", globals()
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_EXIT = _require_intrinsic(
    "molt_contextlib_async_exitstack_exit", globals()
)


def _copy_wrapper_metadata(
    wrapper: Callable[..., Any], wrapped: Callable[..., Any]
) -> None:
    for name in (
        "__module__",
        "__name__",
        "__qualname__",
        "__doc__",
        "__annotations__",
    ):
        try:
            setattr(wrapper, name, getattr(wrapped, name))
        except Exception:
            pass
    try:
        wrapper.__wrapped__ = wrapped  # type: ignore[attr-defined]
    except Exception:
        pass


def nullcontext(value: Any = None) -> Any:
    return _MOLT_CONTEXT_NULL(value)


def closing(thing: Any) -> Any:
    return _MOLT_CONTEXTLIB_CLOSING(thing)


class _AClosing:
    def __init__(self, thing: Any) -> None:
        self._thing = thing

    async def __aenter__(self) -> Any:
        return _MOLT_CONTEXTLIB_ACLOSING_ENTER(self._thing)

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        await _MOLT_CONTEXTLIB_ACLOSING_EXIT(self._thing)
        return False


def aclosing(thing: Any) -> Any:
    return _AClosing(thing)


class ContextDecorator:
    def _recreate_cm(self) -> "ContextDecorator":
        return self

    def __enter__(self) -> Any:
        raise NotImplementedError

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        raise NotImplementedError

    def __call__(self, func: Callable[..., Any]) -> Callable[..., Any]:
        def _inner(*args: Any, **kwargs: Any) -> Any:
            with self._recreate_cm():
                return func(*args, **kwargs)

        _copy_wrapper_metadata(_inner, func)
        return _inner


class _GeneratorContextManager(ContextDecorator):
    def __init__(
        self, func: Callable[..., Any], args: tuple[Any, ...], kwds: dict[str, Any]
    ):
        self._func = func
        self._args = args
        self._kwds = kwds
        self._gen = None

    def _recreate_cm(self) -> "_GeneratorContextManager":
        return _GeneratorContextManager(self._func, self._args, self._kwds)

    def __enter__(self) -> Any:
        if self._gen is None:
            self._gen = self._func(*self._args, **self._kwds)
        return _MOLT_CONTEXTLIB_GENERATOR_ENTER(self._gen)

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._gen is None:
            return False
        return _MOLT_CONTEXTLIB_GENERATOR_EXIT(self._gen, exc_type, exc, tb)


def contextmanager(
    func: Callable[..., Any],
) -> Callable[..., _GeneratorContextManager]:
    def helper(*args: Any, **kwds: Any) -> _GeneratorContextManager:
        return _GeneratorContextManager(func, args, kwds)

    _copy_wrapper_metadata(helper, func)
    return helper


class _AsyncGeneratorContextManager:
    def __init__(
        self, func: Callable[..., Any], args: tuple[Any, ...], kwds: dict[str, Any]
    ):
        self._molt_handle = _MOLT_CONTEXTLIB_ASYNCGEN_CM_NEW(func, args, kwds)

    def __del__(self) -> None:
        try:
            _MOLT_CONTEXTLIB_ASYNCGEN_CM_DROP(self._molt_handle)
        except Exception:
            pass

    async def __aenter__(self) -> Any:
        return await _MOLT_CONTEXTLIB_ASYNCGEN_CM_AENTER(self._molt_handle)

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return await _MOLT_CONTEXTLIB_ASYNCGEN_CM_AEXIT(
            self._molt_handle, exc_type, exc, tb
        )


def asynccontextmanager(
    func: Callable[..., Any],
) -> Callable[..., _AsyncGeneratorContextManager]:
    def helper(*args: Any, **kwds: Any) -> _AsyncGeneratorContextManager:
        return _AsyncGeneratorContextManager(func, args, kwds)

    _copy_wrapper_metadata(helper, func)
    return helper


class AsyncExitStack:
    def __init__(self) -> None:
        self._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_NEW()

    def __del__(self) -> None:
        try:
            _MOLT_CONTEXTLIB_EXITSTACK_DROP(self._molt_state)
        except Exception:
            pass

    async def __aenter__(self) -> "AsyncExitStack":
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return await _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_EXIT(
            self._molt_state, exc_type, exc, tb
        )

    async def aclose(self) -> None:
        await self.__aexit__(None, None, None)

    def pop_all(self) -> "AsyncExitStack":
        new_stack = AsyncExitStack.__new__(AsyncExitStack)
        new_stack._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_POP_ALL(self._molt_state)
        return new_stack

    def push_async_exit(
        self, exit: Callable[[Any, Any, Any], Any]
    ) -> Callable[[Any, Any, Any], Any]:
        return _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_EXIT(self._molt_state, exit)

    def push_async_callback(
        self, callback: Callable[..., Any], *args: Any, **kwds: Any
    ) -> Callable[..., Any]:
        _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_CALLBACK(
            self._molt_state, callback, args, kwds
        )
        return callback

    async def enter_async_context(self, cm: Any) -> Any:
        return await _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT(
            self._molt_state, cm
        )


class ExitStack(ContextDecorator):
    def __init__(self) -> None:
        self._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_NEW()

    def __del__(self) -> None:
        try:
            _MOLT_CONTEXTLIB_EXITSTACK_DROP(self._molt_state)
        except Exception:
            pass

    def __enter__(self) -> "ExitStack":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return _MOLT_CONTEXTLIB_EXITSTACK_EXIT(self._molt_state, exc_type, exc, tb)

    def close(self) -> None:
        self.__exit__(None, None, None)

    def pop_all(self) -> "ExitStack":
        new_stack = ExitStack.__new__(ExitStack)
        new_stack._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_POP_ALL(self._molt_state)
        return new_stack

    def push(
        self, exit: Callable[[Any, Any, Any], Any]
    ) -> Callable[[Any, Any, Any], Any]:
        _MOLT_CONTEXTLIB_EXITSTACK_PUSH(self._molt_state, exit)
        return exit

    def callback(self, callback: Callable[..., Any], *args: Any, **kwds: Any) -> None:
        _MOLT_CONTEXTLIB_EXITSTACK_PUSH_CALLBACK(self._molt_state, callback, args, kwds)

    def enter_context(self, cm: Any) -> Any:
        return _MOLT_CONTEXTLIB_EXITSTACK_ENTER_CONTEXT(self._molt_state, cm)


class suppress(ContextDecorator):
    def __init__(self, *exceptions: type[BaseException]) -> None:
        self._exceptions = exceptions

    def __enter__(self) -> None:
        return None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if exc_type is None:
            return False
        return bool(_MOLT_CONTEXTLIB_SUPPRESS_MATCH(exc_type, self._exceptions))


class _RedirectStream(ContextDecorator):
    def __init__(self, new_target: Any, stream_name: str) -> None:
        self._new_target = new_target
        self._stream = stream_name
        self._old_target = None

    def __enter__(self) -> Any:
        self._old_target = _MOLT_CONTEXTLIB_REDIRECT_ENTER(
            _sys, self._stream, self._new_target
        )
        return self._new_target

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        _MOLT_CONTEXTLIB_REDIRECT_EXIT(_sys, self._stream, self._old_target)
        return False


class redirect_stdout(_RedirectStream):
    def __init__(self, new_target: Any) -> None:
        super().__init__(new_target, "stdout")


class redirect_stderr(_RedirectStream):
    def __init__(self, new_target: Any) -> None:
        super().__init__(new_target, "stderr")
