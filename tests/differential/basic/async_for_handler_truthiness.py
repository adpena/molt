"""Purpose: an `async for` ends when its iterator raises StopAsyncIteration.

The loop's handler tests its StopAsyncIteration match while that exception is
still pending. The truth test of the match result must not read the pending
exception as its own failure; on 2026-11-15 the runtime's truth test did, and
every `async for` over an async generator leaked StopAsyncIteration on LLVM.
"""

import asyncio


async def agen(n):
    for i in range(n):
        yield i


class Counter:
    def __init__(self, limit):
        self.limit = limit
        self.value = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.value >= self.limit:
            raise StopAsyncIteration
        self.value += 1
        await asyncio.sleep(0)
        return self.value


async def main():
    out = []
    async for y in agen(3):
        out.append(y)
    print(out)
    print([v async for v in Counter(3)])
    try:
        raise KeyError("pending")
    except KeyError as exc:
        # A truth test inside a handler, with the handled exception live.
        print(bool([v async for v in agen(2)]), type(exc).__name__)


asyncio.run(main())
