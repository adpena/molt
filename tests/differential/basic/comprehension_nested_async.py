"""Purpose: differential coverage for nested async comprehensions."""

import asyncio


class AsyncCounter:
    def __init__(self, limit: int) -> None:
        self._limit = limit
        self._cur = 0

    def __aiter__(self) -> "AsyncCounter":
        return self

    async def __anext__(self) -> int:
        if self._cur >= self._limit:
            raise StopAsyncIteration
        value = self._cur
        self._cur += 1
        await asyncio.sleep(0)
        return value


async def main() -> None:
    nested = [[y async for y in AsyncCounter(2)] async for _ in AsyncCounter(2)]
    print(nested)


asyncio.run(main())
