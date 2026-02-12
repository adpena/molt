"""Purpose: differential coverage for asyncio queue join."""

import asyncio


async def main() -> None:
    q: asyncio.Queue[int] = asyncio.Queue()
    await q.put(1)
    await q.put(2)

    got: list[int] = []

    async def worker() -> None:
        while not q.empty():
            item = await q.get()
            got.append(item)
            q.task_done()

    await worker()
    await q.join()
    print(sorted(got))


asyncio.run(main())
