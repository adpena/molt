import asyncio


class AsyncIter:
    def __init__(self) -> None:
        self.i = 0

    def __aiter__(self) -> "AsyncIter":
        return self

    async def __anext__(self) -> int:
        if self.i >= 1:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        return val


async def main() -> int:
    it = AsyncIter()
    fut = anext(it, 10)
    first = await fut
    second = await anext(it, 20)
    return first + second


print(asyncio.run(main()))
