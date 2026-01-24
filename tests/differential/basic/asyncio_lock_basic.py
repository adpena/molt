"""Purpose: differential coverage for asyncio lock basic."""

import asyncio


async def main() -> None:
    lock = asyncio.Lock()
    order: list[str] = []
    start = asyncio.Event()
    release = asyncio.Event()

    async def first() -> None:
        await lock.acquire()
        order.append("first")
        start.set()
        await release.wait()
        lock.release()

    async def second() -> None:
        await start.wait()
        await lock.acquire()
        order.append("second")
        lock.release()

    t1 = asyncio.create_task(first())
    t2 = asyncio.create_task(second())
    await asyncio.sleep(0)
    release.set()
    await asyncio.gather(t1, t2)
    print(order)


asyncio.run(main())
