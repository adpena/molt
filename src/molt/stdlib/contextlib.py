"""Context manager helpers for Molt (capability-safe subset)."""

from __future__ import annotations

from typing import Any, Callable

import functools
import inspect as _inspect
import sys as _sys

__all__ = [
    "ContextDecorator",
    "AsyncExitStack",
    "ExitStack",
    "contextmanager",
    "asynccontextmanager",
    "closing",
    "nullcontext",
    "redirect_stderr",
    "redirect_stdout",
    "suppress",
]


class _NullContext:
    def __init__(self, value: Any = None) -> None:
        self._value = value

    def __enter__(self) -> Any:
        return self._value

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return False


def nullcontext(value: Any = None) -> _NullContext:
    return _NullContext(value)


class _Closing:
    def __init__(self, thing: Any) -> None:
        self._thing = thing

    def __enter__(self) -> Any:
        return self._thing

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        close = getattr(self._thing, "close", None)
        if callable(close):
            close()
        return False


def closing(thing: Any) -> _Closing:
    return _Closing(thing)


class ContextDecorator:
    def _recreate_cm(self) -> "ContextDecorator":
        return self

    def __enter__(self) -> Any:
        raise NotImplementedError

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        raise NotImplementedError

    def __call__(self, func: Callable[..., Any]) -> Callable[..., Any]:
        @functools.wraps(func)
        def _inner(*args: Any, **kwargs: Any) -> Any:
            with self._recreate_cm():
                return func(*args, **kwargs)

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
        try:
            return next(self._gen)
        except StopIteration:
            raise RuntimeError("generator didn't yield") from None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._gen is None:
            return False
        if exc_type is None:
            try:
                next(self._gen)
            except StopIteration:
                return False
            else:
                raise RuntimeError("generator didn't stop")
        if exc is None:
            exc = exc_type()
        if tb is not None:
            try:
                exc.__traceback__ = tb
            except Exception:
                pass
        try:
            self._gen.throw(exc)
        except StopIteration as stop:
            return stop is not exc
        except RuntimeError as err:
            if err is exc:
                return False
            raise
        except BaseException:
            if exc is None:
                raise
            return False
        else:
            raise RuntimeError("generator didn't stop after throw")


def contextmanager(
    func: Callable[..., Any],
) -> Callable[..., _GeneratorContextManager]:
    @functools.wraps(func)
    def helper(*args: Any, **kwds: Any) -> _GeneratorContextManager:
        return _GeneratorContextManager(func, args, kwds)

    return helper


class _AsyncGeneratorContextManager:
    def __init__(
        self, func: Callable[..., Any], args: tuple[Any, ...], kwds: dict[str, Any]
    ):
        self._func = func
        self._args = args
        self._kwds = kwds
        self._agen = None

    async def __aenter__(self) -> Any:
        if self._agen is None:
            self._agen = self._func(*self._args, **self._kwds)
        try:
            return await self._agen.__anext__()
        except StopAsyncIteration:
            raise RuntimeError("async generator didn't yield") from None

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._agen is None:
            return False
        if exc_type is None:
            try:
                await self._agen.__anext__()
            except StopAsyncIteration:
                return False
            else:
                raise RuntimeError("async generator didn't stop")
        if exc is None:
            exc = exc_type()
        if tb is not None:
            try:
                exc.__traceback__ = tb
            except Exception:
                pass
        try:
            await self._agen.athrow(exc)
        except StopAsyncIteration as stop:
            return stop is not exc
        except RuntimeError as err:
            if err is exc:
                return False
            raise
        except BaseException:
            if exc is None:
                raise
            return False
        else:
            raise RuntimeError("async generator didn't stop after athrow")


def asynccontextmanager(
    func: Callable[..., Any],
) -> Callable[..., _AsyncGeneratorContextManager]:
    @functools.wraps(func)
    def helper(*args: Any, **kwds: Any) -> _AsyncGeneratorContextManager:
        return _AsyncGeneratorContextManager(func, args, kwds)

    return helper


class AsyncExitStack:
    def __init__(self) -> None:
        self._exit_callbacks: list[Callable[[Any, Any, Any], Any]] = []

    async def __aenter__(self) -> "AsyncExitStack":
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        received_exc = exc_type is not None
        suppressed = False
        while self._exit_callbacks:
            cb = self._exit_callbacks.pop()
            try:
                res = cb(exc_type, exc, tb)
                if _inspect.isawaitable(res):
                    res = await res
                if res:
                    exc_type = exc = tb = None
                    suppressed = True
            except BaseException as new_exc:
                exc_type, exc, tb = type(new_exc), new_exc, new_exc.__traceback__
                suppressed = False
        if received_exc and exc_type is None:
            return True
        if exc_type is None:
            return suppressed
        return False

    async def aclose(self) -> None:
        await self.__aexit__(None, None, None)

    def pop_all(self) -> "AsyncExitStack":
        new_stack = AsyncExitStack()
        new_stack._exit_callbacks = self._exit_callbacks
        self._exit_callbacks = []
        return new_stack

    def push_async_exit(
        self, exit: Callable[[Any, Any, Any], Any]
    ) -> Callable[[Any, Any, Any], Any]:
        self._exit_callbacks.append(exit)
        return exit

    def push_async_callback(
        self, callback: Callable[..., Any], *args: Any, **kwds: Any
    ) -> None:
        async def _exit(_: Any, __: Any, ___: Any) -> bool:
            res = callback(*args, **kwds)
            if _inspect.isawaitable(res):
                await res
            return False

        self._exit_callbacks.append(_exit)

    async def enter_async_context(self, cm: Any) -> Any:
        result = await cm.__aenter__()
        self._exit_callbacks.append(cm.__aexit__)
        return result


class ExitStack(ContextDecorator):
    def __init__(self) -> None:
        self._exit_callbacks: list[Callable[[Any, Any, Any], Any]] = []

    def __enter__(self) -> "ExitStack":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        received_exc = exc_type is not None
        suppressed = False
        while self._exit_callbacks:
            cb = self._exit_callbacks.pop()
            try:
                res = cb(exc_type, exc, tb)
                if res:
                    exc_type = exc = tb = None
                    suppressed = True
            except BaseException as new_exc:
                exc_type, exc, tb = type(new_exc), new_exc, new_exc.__traceback__
                suppressed = False
        if received_exc and exc_type is None:
            return True
        if exc_type is None:
            return suppressed
        return False

    def close(self) -> None:
        self.__exit__(None, None, None)

    def pop_all(self) -> "ExitStack":
        new_stack = ExitStack()
        new_stack._exit_callbacks = self._exit_callbacks
        self._exit_callbacks = []
        return new_stack

    def push(
        self, exit: Callable[[Any, Any, Any], Any]
    ) -> Callable[[Any, Any, Any], Any]:
        self._exit_callbacks.append(exit)
        return exit

    def callback(self, callback: Callable[..., Any], *args: Any, **kwds: Any) -> None:
        def _exit(_: Any, __: Any, ___: Any) -> bool:
            callback(*args, **kwds)
            return False

        self._exit_callbacks.append(_exit)

    def enter_context(self, cm: Any) -> Any:
        result = cm.__enter__()
        self._exit_callbacks.append(cm.__exit__)
        return result


class suppress(ContextDecorator):
    def __init__(self, *exceptions: type[BaseException]) -> None:
        self._exceptions = exceptions

    def __enter__(self) -> None:
        return None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if exc_type is None:
            return False
        return issubclass(exc_type, self._exceptions)


class _RedirectStream(ContextDecorator):
    def __init__(self, new_target: Any, stream_name: str) -> None:
        self._new_target = new_target
        self._stream = stream_name
        self._old_target = None

    def __enter__(self) -> Any:
        self._old_target = getattr(_sys, self._stream)
        setattr(_sys, self._stream, self._new_target)
        return self._new_target

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        setattr(_sys, self._stream, self._old_target)
        return False


class redirect_stdout(_RedirectStream):
    def __init__(self, new_target: Any) -> None:
        super().__init__(new_target, "stdout")


class redirect_stderr(_RedirectStream):
    def __init__(self, new_target: Any) -> None:
        super().__init__(new_target, "stderr")
