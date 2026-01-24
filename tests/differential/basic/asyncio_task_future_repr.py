"""Purpose: differential coverage for asyncio task future repr."""

import asyncio


async def main() -> None:
    fut = asyncio.Future()
    task = asyncio.create_task(asyncio.sleep(0))
    print("Future" in repr(fut))
    print("Task" in repr(task))
    await task


asyncio.run(main())
