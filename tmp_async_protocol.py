import asyncio


class Counter:
    def __init__(self, n):
        self.i = 1
        self.n = n

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.i > self.n:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        await asyncio.sleep(0)
        return val


async def main():
    async for item in Counter(3):
        print(item)
    async for item in [20, 30]:
        print(item)
    it = aiter(Counter(1))
    print(await anext(it))
    try:
        await anext(it)
    except StopAsyncIteration:
        print("done")
    it2 = aiter(Counter(0))
    print(await anext(it2, 7))


asyncio.run(main())
