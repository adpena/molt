# Shim churn audit: 1 intrinsic-direct / 14 total exports
"""Intrinsic-backed context manager helpers for Molt.

Pure-forwarding shims eliminated per MOL-215 where argument signatures permit.
"""

from __future__ import annotations

import abc
from typing import Any as _Any, Callable as _Callable
from types import GenericAlias, MethodType
from collections import deque
from functools import wraps
import os

import sys

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "AbstractAsyncContextManager",
    "AbstractContextManager",
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
    "chdir",
]


_MOLT_CONTEXTLIB_ABSTRACT_ENTER = _require_intrinsic("molt_contextlib_abstract_enter")
_MOLT_CONTEXTLIB_ABSTRACT_AENTER = _require_intrinsic("molt_contextlib_abstract_aenter")
_MOLT_CONTEXTLIB_ABSTRACT_SUBCLASSHOOK = _require_intrinsic(
    "molt_contextlib_abstract_subclasshook"
)
_MOLT_CONTEXTLIB_ABSTRACT_ASYNC_SUBCLASSHOOK = _require_intrinsic(
    "molt_contextlib_abstract_async_subclasshook"
)
_MOLT_CONTEXTLIB_CONTEXTDECORATOR_CALL = _require_intrinsic(
    "molt_contextlib_contextdecorator_call"
)
_MOLT_CONTEXTLIB_CHDIR_ENTER = _require_intrinsic("molt_contextlib_chdir_enter")
_MOLT_CONTEXTLIB_CHDIR_EXIT = _require_intrinsic("molt_contextlib_chdir_exit")
_MOLT_CONTEXTLIB_ASYNCGEN_ENTER = _require_intrinsic("molt_contextlib_asyncgen_enter")
_MOLT_CONTEXTLIB_ASYNCGEN_EXIT = _require_intrinsic("molt_contextlib_asyncgen_exit")
_MOLT_CONTEXTLIB_GENERATOR_ENTER = _require_intrinsic("molt_contextlib_generator_enter")
_MOLT_CONTEXTLIB_GENERATOR_EXIT = _require_intrinsic("molt_contextlib_generator_exit")
_MOLT_CONTEXTLIB_SUPPRESS_MATCH = _require_intrinsic("molt_contextlib_suppress_match")
_MOLT_CONTEXTLIB_REDIRECT_ENTER = _require_intrinsic("molt_contextlib_redirect_enter")
_MOLT_CONTEXTLIB_REDIRECT_EXIT = _require_intrinsic("molt_contextlib_redirect_exit")
_MOLT_CONTEXTLIB_EXITSTACK_NEW = _require_intrinsic("molt_contextlib_exitstack_new")
_MOLT_CONTEXTLIB_EXITSTACK_DROP = _require_intrinsic("molt_contextlib_exitstack_drop")
_MOLT_CONTEXTLIB_EXITSTACK_PUSH = _require_intrinsic("molt_contextlib_exitstack_push")
_MOLT_CONTEXTLIB_EXITSTACK_PUSH_CALLBACK = _require_intrinsic(
    "molt_contextlib_exitstack_push_callback"
)
_MOLT_CONTEXTLIB_EXITSTACK_POP_ALL = _require_intrinsic(
    "molt_contextlib_exitstack_pop_all"
)
_MOLT_CONTEXTLIB_EXITSTACK_EXIT = _require_intrinsic("molt_contextlib_exitstack_exit")
_MOLT_CONTEXTLIB_EXITSTACK_ENTER_CONTEXT = _require_intrinsic(
    "molt_contextlib_exitstack_enter_context"
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_CALLBACK = _require_intrinsic(
    "molt_contextlib_async_exitstack_push_callback"
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_EXIT = _require_intrinsic(
    "molt_contextlib_async_exitstack_push_exit"
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT = _require_intrinsic(
    "molt_contextlib_async_exitstack_enter_context"
)
_MOLT_CONTEXTLIB_ASYNC_EXITSTACK_EXIT = _require_intrinsic(
    "molt_contextlib_async_exitstack_exit"
)


def _copy_wrapper_metadata(
    wrapper: _Callable[..., _Any], wrapped: _Callable[..., _Any]
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


class AbstractContextManager(abc.ABC):
    __class_getitem__ = classmethod(GenericAlias)
    __slots__ = ()

    def __enter__(self) -> _Any:
        return _MOLT_CONTEXTLIB_ABSTRACT_ENTER(self)

    @abc.abstractmethod
    def __exit__(self, exc_type: _Any, exc_value: _Any, traceback: _Any) -> _Any:
        return None

    @classmethod
    def __subclasshook__(cls, C: _Any) -> _Any:
        if cls is AbstractContextManager:
            return _MOLT_CONTEXTLIB_ABSTRACT_SUBCLASSHOOK(C)
        return NotImplemented


class AbstractAsyncContextManager(abc.ABC):
    __class_getitem__ = classmethod(GenericAlias)
    __slots__ = ()

    async def __aenter__(self) -> _Any:
        return _MOLT_CONTEXTLIB_ABSTRACT_AENTER(self)

    @abc.abstractmethod
    async def __aexit__(self, exc_type: _Any, exc_value: _Any, traceback: _Any) -> _Any:
        return None

    @classmethod
    def __subclasshook__(cls, C: _Any) -> _Any:
        if cls is AbstractAsyncContextManager:
            return _MOLT_CONTEXTLIB_ABSTRACT_ASYNC_SUBCLASSHOOK(C)
        return NotImplemented


class closing(AbstractContextManager):
    def __init__(self, thing):
        self.thing = thing

    def __enter__(self):
        return self.thing

    def __exit__(self, *exc_info):
        self.thing.close()


class aclosing(AbstractAsyncContextManager):
    def __init__(self, thing):
        self.thing = thing

    async def __aenter__(self):
        return self.thing

    async def __aexit__(self, *exc_info):
        await self.thing.aclose()


class nullcontext(AbstractContextManager, AbstractAsyncContextManager):
    def __init__(self, enter_result=None):
        self.enter_result = enter_result

    def __enter__(self):
        return self.enter_result

    def __exit__(self, *exc_info):
        pass

    async def __aenter__(self):
        return self.enter_result

    async def __aexit__(self, *exc_info):
        pass


class ContextDecorator:
    def _recreate_cm(self) -> "ContextDecorator":
        return self

    def __call__(self, func: _Callable[..., _Any]) -> _Callable[..., _Any]:
        def _inner(*args: _Any, **kwargs: _Any) -> _Any:
            cm = self._recreate_cm()
            return _MOLT_CONTEXTLIB_CONTEXTDECORATOR_CALL(cm, func, args, kwargs)

        _copy_wrapper_metadata(_inner, func)
        return _inner


class AsyncContextDecorator:
    def _recreate_cm(self) -> "AsyncContextDecorator":
        return self

    def __call__(self, func: _Callable[..., _Any]) -> _Callable[..., _Any]:
        async def _inner(*args: _Any, **kwds: _Any) -> _Any:
            async with self._recreate_cm():
                return await func(*args, **kwds)

        _copy_wrapper_metadata(_inner, func)
        return _inner


class _GeneratorContextManager(ContextDecorator):
    def __init__(
        self, func: _Callable[..., _Any], args: tuple[_Any, ...], kwds: dict[str, _Any]
    ):
        self._func = func
        self._args = args
        self._kwds = kwds
        self._gen = None

    def _recreate_cm(self) -> "_GeneratorContextManager":
        return _GeneratorContextManager(self._func, self._args, self._kwds)

    def __enter__(self) -> _Any:
        if self._gen is None:
            self._gen = self._func(*self._args, **self._kwds)
        return _MOLT_CONTEXTLIB_GENERATOR_ENTER(self._gen)

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        if self._gen is None:
            return False
        return _MOLT_CONTEXTLIB_GENERATOR_EXIT(self._gen, exc_type, exc, tb)


def contextmanager(
    func: _Callable[..., _Any],
) -> _Callable[..., _GeneratorContextManager]:
    def helper(*args: _Any, **kwds: _Any) -> _GeneratorContextManager:
        return _GeneratorContextManager(func, args, kwds)

    _copy_wrapper_metadata(helper, func)
    return helper


class _AsyncGeneratorContextManager(AsyncContextDecorator):
    def __init__(
        self, func: _Callable[..., _Any], args: tuple[_Any, ...], kwds: dict[str, _Any]
    ):
        self._func = func
        self._args = args
        self._kwds = kwds
        self._agen = None

    def _recreate_cm(self) -> "_AsyncGeneratorContextManager":
        return _AsyncGeneratorContextManager(self._func, self._args, self._kwds)

    async def __aenter__(self) -> _Any:
        if self._agen is None:
            self._agen = self._func(*self._args, **self._kwds)
        return await _MOLT_CONTEXTLIB_ASYNCGEN_ENTER(self._agen)

    async def __aexit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        if self._agen is None:
            return False
        return await _MOLT_CONTEXTLIB_ASYNCGEN_EXIT(self._agen, exc_type, exc, tb)


def asynccontextmanager(
    func: _Callable[..., _Any],
) -> _Callable[..., _AsyncGeneratorContextManager]:
    def helper(*args: _Any, **kwds: _Any) -> _AsyncGeneratorContextManager:
        return _AsyncGeneratorContextManager(func, args, kwds)

    _copy_wrapper_metadata(helper, func)
    return helper


class AsyncExitStack(AbstractAsyncContextManager):
    def __init__(self) -> None:
        self._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_NEW()

    def __del__(self, _drop=_MOLT_CONTEXTLIB_EXITSTACK_DROP) -> None:
        try:
            handle = getattr(self, '_molt_state', None)
            if handle is None:
                return
            object.__setattr__(self, '_molt_state', None)
            _drop(handle)
        except Exception:
            pass

    async def __aenter__(self) -> "AsyncExitStack":
        return self

    async def __aexit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
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
        self, exit: _Callable[[_Any, _Any, _Any], _Any]
    ) -> _Callable[[_Any, _Any, _Any], _Any]:
        return _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_EXIT(self._molt_state, exit)

    def push_async_callback(
        self, callback: _Callable[..., _Any], *args: _Any, **kwds: _Any
    ) -> _Callable[..., _Any]:
        _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_PUSH_CALLBACK(
            self._molt_state, callback, args, kwds
        )
        return callback

    async def enter_async_context(self, cm: _Any) -> _Any:
        return await _MOLT_CONTEXTLIB_ASYNC_EXITSTACK_ENTER_CONTEXT(
            self._molt_state, cm
        )


class ExitStack(AbstractContextManager):
    def __init__(self) -> None:
        self._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_NEW()

    def __del__(self, _drop=_MOLT_CONTEXTLIB_EXITSTACK_DROP) -> None:
        try:
            handle = getattr(self, '_molt_state', None)
            if handle is None:
                return
            object.__setattr__(self, '_molt_state', None)
            _drop(handle)
        except Exception:
            pass

    def __enter__(self) -> "ExitStack":
        return self

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        return _MOLT_CONTEXTLIB_EXITSTACK_EXIT(self._molt_state, exc_type, exc, tb)

    def close(self) -> None:
        self.__exit__(None, None, None)

    def pop_all(self) -> "ExitStack":
        new_stack = ExitStack.__new__(ExitStack)
        new_stack._molt_state = _MOLT_CONTEXTLIB_EXITSTACK_POP_ALL(self._molt_state)
        return new_stack

    def push(
        self, exit: _Callable[[_Any, _Any, _Any], _Any]
    ) -> _Callable[[_Any, _Any, _Any], _Any]:
        _MOLT_CONTEXTLIB_EXITSTACK_PUSH(self._molt_state, exit)
        return exit

    def callback(self, callback: _Callable[..., _Any], *args: _Any, **kwds: _Any) -> None:
        _MOLT_CONTEXTLIB_EXITSTACK_PUSH_CALLBACK(self._molt_state, callback, args, kwds)

    def enter_context(self, cm: _Any) -> _Any:
        return _MOLT_CONTEXTLIB_EXITSTACK_ENTER_CONTEXT(self._molt_state, cm)


class suppress(AbstractContextManager):
    def __init__(self, *exceptions: type[BaseException]) -> None:
        self._exceptions = exceptions

    def __enter__(self) -> None:
        return None

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        if exc_type is None:
            return False
        return bool(_MOLT_CONTEXTLIB_SUPPRESS_MATCH(exc_type, self._exceptions))


class _RedirectStream(AbstractContextManager):
    def __init__(self, new_target: _Any, stream_name: str) -> None:
        self._new_target = new_target
        self._stream = stream_name
        self._old_target = None

    def __enter__(self) -> _Any:
        self._old_target = _MOLT_CONTEXTLIB_REDIRECT_ENTER(
            sys, self._stream, self._new_target
        )
        return self._new_target

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        _MOLT_CONTEXTLIB_REDIRECT_EXIT(sys, self._stream, self._old_target)
        return False


class redirect_stdout(_RedirectStream):
    def __init__(self, new_target: _Any) -> None:
        super().__init__(new_target, "stdout")


class redirect_stderr(_RedirectStream):
    def __init__(self, new_target: _Any) -> None:
        super().__init__(new_target, "stderr")


class chdir(AbstractContextManager):
    def __init__(self, path: _Any) -> None:
        self.path = path
        self._old_cwd: list[str] = []

    def __enter__(self) -> None:
        old_cwd = _MOLT_CONTEXTLIB_CHDIR_ENTER(self.path)
        self._old_cwd.append(old_cwd)
        return None

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> bool:
        _MOLT_CONTEXTLIB_CHDIR_EXIT(self._old_cwd.pop())
        return False


globals().pop("_require_intrinsic", None)
