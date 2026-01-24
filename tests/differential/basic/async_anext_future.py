"""Purpose: differential coverage for async anext future."""

import asyncio


class AsyncIter:
    def __init__(self) -> None:
        self.i = 0

    def __aiter__(self) -> "AsyncIter":
        return self

    async def __anext__(self) -> int:
        if self.i >= 2:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        return val


async def main() -> int:
    iterator = AsyncIter()
    fut0 = anext(iterator)
    v0 = await fut0
    fut1 = anext(iterator)
    v1 = await fut1
    return v0 + v1


print(asyncio.run(main()))
