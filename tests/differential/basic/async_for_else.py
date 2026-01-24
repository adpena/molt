"""Purpose: differential coverage for async for else."""

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


async def main() -> list[int | str]:
    out: list[int | str] = []
    async for value in AsyncIter():
        out.append(value)
    else:
        out.append("done")
    out2: list[int | str] = []
    async for value in AsyncIter():
        if value == 1:
            break
        out2.append(value)
    else:
        out2.append("done")
    return [out, out2]


print(asyncio.run(main()))
