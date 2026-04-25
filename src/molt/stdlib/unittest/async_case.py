"""IsolatedAsyncioTestCase — async support for unittest.

Direct port of CPython 3.12's unittest/async_case.py. Pure Python;
relies on `asyncio` (intrinsic-backed in molt), `contextvars`,
`inspect`, and `warnings`. Used by any test suite that exercises async
code via `unittest`.
"""

import asyncio
import contextvars
import inspect
import warnings

from .case import TestCase

__unittest = True


class IsolatedAsyncioTestCase(TestCase):
    def __init__(self, methodName='runTest'):
        super().__init__(methodName)
        self._asyncioRunner = None
        self._asyncioTestContext = contextvars.copy_context()

    async def asyncSetUp(self):
        pass

    async def asyncTearDown(self):
        pass

    def addAsyncCleanup(self, func, /, *args, **kwargs):
        # A trivial trampoline to addCleanup() that documents the async
        # semantics; addCleanup() runs regular functions, addAsyncCleanup
        # runs coroutines via the same path.
        self.addCleanup(*(func, *args), **kwargs)

    async def enterAsyncContext(self, cm):
        """Enter the supplied asynchronous context manager and register
        its __aexit__ as a cleanup."""
        cls = type(cm)
        try:
            enter = cls.__aenter__
            exit = cls.__aexit__
        except AttributeError:
            raise TypeError(
                f"'{cls.__module__}.{cls.__qualname__}' object does "
                f"not support the asynchronous context manager protocol"
            ) from None
        result = await enter(cm)
        self.addAsyncCleanup(exit, cm, None, None, None)
        return result

    def _callSetUp(self):
        self._asyncioRunner.get_loop()
        self._asyncioTestContext.run(self.setUp)
        self._callAsync(self.asyncSetUp)

    def _callTestMethod(self, method):
        if self._callMaybeAsync(method) is not None:
            warnings.warn(
                f'It is deprecated to return a value that is not None from a '
                f'test case ({method})', DeprecationWarning, stacklevel=4,
            )

    def _callTearDown(self):
        self._callAsync(self.asyncTearDown)
        self._asyncioTestContext.run(self.tearDown)

    def _callCleanup(self, function, *args, **kwargs):
        self._callMaybeAsync(function, *args, **kwargs)

    def _callAsync(self, func, /, *args, **kwargs):
        assert self._asyncioRunner is not None, 'asyncio runner is not initialized'
        assert inspect.iscoroutinefunction(func), f'{func!r} is not an async function'
        return self._asyncioRunner.run(
            func(*args, **kwargs),
            context=self._asyncioTestContext,
        )

    def _callMaybeAsync(self, func, /, *args, **kwargs):
        assert self._asyncioRunner is not None, 'asyncio runner is not initialized'
        if inspect.iscoroutinefunction(func):
            return self._asyncioRunner.run(
                func(*args, **kwargs),
                context=self._asyncioTestContext,
            )
        return self._asyncioTestContext.run(func, *args, **kwargs)

    def _setupAsyncioRunner(self):
        assert self._asyncioRunner is None, 'asyncio runner is already initialized'
        runner = asyncio.Runner(debug=True)
        self._asyncioRunner = runner

    def _tearDownAsyncioRunner(self):
        runner = self._asyncioRunner
        runner.close()

    def run(self, result=None):
        self._setupAsyncioRunner()
        try:
            return super().run(result)
        finally:
            self._tearDownAsyncioRunner()

    def debug(self):
        self._setupAsyncioRunner()
        super().debug()
        self._tearDownAsyncioRunner()

    def __del__(self):
        if self._asyncioRunner is not None:
            self._tearDownAsyncioRunner()
