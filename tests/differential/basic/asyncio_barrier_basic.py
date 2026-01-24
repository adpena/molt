"""Purpose: differential coverage for asyncio.Barrier basic."""

import asyncio


async def worker(barrier: asyncio.Barrier, log: list[int]) -> None:
    value = await barrier.wait()
    log.append(value)


async def main() -> None:
    barrier = asyncio.Barrier(2)
    log: list[int] = []
    t1 = asyncio.create_task(worker(barrier, log))
    t2 = asyncio.create_task(worker(barrier, log))
    await asyncio.gather(t1, t2)
    print(sorted(log))


asyncio.run(main())
