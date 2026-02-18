"""Purpose: regression for async-for over temporary iterables returning self from __aiter__."""

import asyncio


class Counter:
    def __init__(self, n: int) -> None:
        self.i = 0
        self.n = n

    def __aiter__(self) -> "Counter":
        return self

    async def __anext__(self) -> int:
        if self.i >= self.n:
            raise StopAsyncIteration
        value = self.i
        self.i += 1
        return value


async def main() -> None:
    values: list[int] = []
    async for value in Counter(3):
        values.append(value)
    print(values)


asyncio.run(main())
