"""Purpose: differential coverage for async for/with exception propagation."""

import asyncio


class AsyncIter:
    def __init__(self):
        self.count = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.count == 2:
            raise RuntimeError("boom")
        self.count += 1
        return self.count


class AsyncCtx:
    async def __aenter__(self):
        return "ok"

    async def __aexit__(self, exc_type, exc, tb):
        return False


async def main():
    values = []
    try:
        async for item in AsyncIter():
            values.append(item)
    except Exception as exc:
        print("async_for", type(exc).__name__, values)

    try:
        async with AsyncCtx():
            raise ValueError("boom")
    except Exception as exc:
        print("async_with", type(exc).__name__)


asyncio.run(main())
