"""Purpose: differential coverage for PriorityQueue and LifoQueue ordering."""

import asyncio


async def main() -> None:
    pq: asyncio.PriorityQueue[tuple[int, str]] = asyncio.PriorityQueue()
    await pq.put((2, "b"))
    await pq.put((1, "a"))
    await pq.put((3, "c"))
    pq_out = [await pq.get(), await pq.get(), await pq.get()]

    lq: asyncio.LifoQueue[str] = asyncio.LifoQueue()
    await lq.put("first")
    await lq.put("second")
    lq_out = [await lq.get(), await lq.get()]
    print(pq_out, lq_out)


asyncio.run(main())
