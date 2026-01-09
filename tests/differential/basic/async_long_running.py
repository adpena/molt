import asyncio


order: list[str] = []


class Counter:
    def __init__(self, n: int) -> None:
        self.i = 0
        self.n = n

    def __aiter__(self) -> "Counter":
        return self

    async def __anext__(self) -> int:
        if self.i >= self.n:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        order.append(f"before-{val}")
        await asyncio.sleep(0)
        order.append(f"after-{val}")
        return val


class AsyncList:
    def __init__(self, items: list[int]) -> None:
        self._items = items
        self._idx = 0

    def __aiter__(self) -> "AsyncList":
        return self

    async def __anext__(self) -> int:
        if self._idx >= len(self._items):
            raise StopAsyncIteration
        val = self._items[self._idx]
        self._idx += 1
        await asyncio.sleep(0)
        return val


async def main() -> None:
    vals: list[int] = []
    async for item in Counter(3):
        vals.append(item)
    print(vals)
    print(order)
    total = 0
    for i in range(3):
        total += i
        await asyncio.sleep(0)
        total += 100
    print(total)
    async_vals: list[int] = []
    async for item in AsyncList([20, 30]):
        async_vals.append(item)
    print(async_vals)


asyncio.run(main())
