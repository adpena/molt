"""Purpose: differential coverage for async comprehensions (PEP 530)."""

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
    data = [value async for value in AsyncCounter(4)]
    evens = [value async for value in AsyncCounter(6) if value % 2 == 0]
    mapping = {value: value * value async for value in AsyncCounter(3)}
    agen = (value async for value in AsyncCounter(3))
    agen_list = [value async for value in agen]
    print(data)
    print(evens)
    print(sorted(mapping.items()))
    print(agen_list)


asyncio.run(main())
