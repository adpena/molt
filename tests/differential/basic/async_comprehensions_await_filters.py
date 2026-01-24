"""Purpose: differential coverage for async comprehensions with await."""

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


async def double(value: int) -> int:
    await asyncio.sleep(0)
    return value * 2


async def main() -> None:
    values = [await double(x) async for x in AsyncCounter(4)]
    filtered = [x async for x in AsyncCounter(6) if (await double(x)) > 4]
    mapping = {x: await double(x) async for x in AsyncCounter(3)}
    print(values)
    print(filtered)
    print(sorted(mapping.items()))


asyncio.run(main())
