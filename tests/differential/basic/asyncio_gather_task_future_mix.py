"""Purpose: differential coverage for asyncio gather task future mix."""

import asyncio


async def child(val: int) -> int:
    await asyncio.sleep(0)
    return val


async def set_future(fut: asyncio.Future) -> None:
    await asyncio.sleep(0)
    fut.set_result(2)


async def main() -> None:
    fut: asyncio.Future = asyncio.Future()
    task = asyncio.create_task(child(1))
    asyncio.create_task(set_future(fut))
    res = await asyncio.gather(task, fut)
    print(res)


asyncio.run(main())
