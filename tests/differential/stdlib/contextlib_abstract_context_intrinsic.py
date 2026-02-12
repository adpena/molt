"""Purpose: verify AbstractContextManager/AbstractAsyncContextManager intrinsic parity."""

import asyncio
import contextlib


class SyncCM:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False


class SyncMissingExit:
    def __enter__(self):
        return self


print(issubclass(SyncCM, contextlib.AbstractContextManager))
print(issubclass(SyncMissingExit, contextlib.AbstractContextManager))
print(contextlib.AbstractContextManager.__subclasshook__(SyncCM))
print(contextlib.AbstractContextManager.__subclasshook__(SyncMissingExit))
try:
    contextlib.AbstractContextManager.__subclasshook__(object())
except Exception as exc:
    print(type(exc).__name__)


class SyncImpl(contextlib.AbstractContextManager):
    def __exit__(self, exc_type, exc, tb):
        return False


sync_impl = SyncImpl()
print(sync_impl.__enter__() is sync_impl)


class AsyncCM:
    async def __aenter__(self):
        return self

    async def __aexit__(self, exc_type, exc, tb):
        return False


class AsyncMissingExit:
    async def __aenter__(self):
        return self


print(issubclass(AsyncCM, contextlib.AbstractAsyncContextManager))
print(issubclass(AsyncMissingExit, contextlib.AbstractAsyncContextManager))
print(contextlib.AbstractAsyncContextManager.__subclasshook__(AsyncCM))
print(contextlib.AbstractAsyncContextManager.__subclasshook__(AsyncMissingExit))
try:
    contextlib.AbstractAsyncContextManager.__subclasshook__(object())
except Exception as exc:
    print(type(exc).__name__)


class AsyncImpl(contextlib.AbstractAsyncContextManager):
    async def __aexit__(self, exc_type, exc, tb):
        return False


async def _run() -> None:
    async_impl = AsyncImpl()
    entered = await async_impl.__aenter__()
    print(entered is async_impl)


asyncio.run(_run())
