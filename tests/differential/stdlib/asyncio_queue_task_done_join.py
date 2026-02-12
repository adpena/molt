"""Purpose: differential coverage for asyncio queue task done join."""

import asyncio


async def main() -> None:
    queue: asyncio.Queue[int] = asyncio.Queue()
    await queue.put(1)
    await queue.put(2)

    got: list[int] = []
    got.append(await queue.get())
    queue.task_done()
    got.append(await queue.get())
    queue.task_done()

    await queue.join()
    print(got)


asyncio.run(main())
