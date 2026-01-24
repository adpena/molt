"""Purpose: differential coverage for async for iter."""

import asyncio


class Counter:
    def __init__(self, n: int) -> None:
        self.n = n

    def __aiter__(self):
        return self

    async def __anext__(self) -> int:
        if self.n <= 0:
            raise StopAsyncIteration
        await asyncio.sleep(0)
        self.n -= 1
        return self.n


async def main() -> int:
    total = 0
    async for value in Counter(3):
        total += value
    return total


print(asyncio.run(main()))
