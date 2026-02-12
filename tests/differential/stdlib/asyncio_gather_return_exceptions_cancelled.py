"""Purpose: differential coverage for asyncio gather return exceptions cancelled."""

import asyncio


async def slow() -> int:
    try:
        await asyncio.sleep(1)
        return 1
    except asyncio.CancelledError:
        return -1


async def main() -> None:
    task = asyncio.create_task(slow())
    await asyncio.sleep(0)
    task.cancel()
    res = await asyncio.gather(task, return_exceptions=True)
    print([type(x).__name__ for x in res])


asyncio.run(main())
