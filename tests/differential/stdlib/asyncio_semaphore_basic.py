"""Purpose: differential coverage for asyncio semaphore basic."""

import asyncio


async def main() -> None:
    sem = asyncio.Semaphore(1)
    order: list[str] = []
    start = asyncio.Event()
    release = asyncio.Event()

    async def first() -> None:
        await sem.acquire()
        order.append("first")
        start.set()
        await release.wait()
        sem.release()

    async def second() -> None:
        await start.wait()
        await sem.acquire()
        order.append("second")
        sem.release()

    t1 = asyncio.create_task(first())
    t2 = asyncio.create_task(second())
    await asyncio.sleep(0)
    release.set()
    await asyncio.gather(t1, t2)
    print(order)


asyncio.run(main())
