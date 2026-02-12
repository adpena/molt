"""Purpose: differential coverage for asyncio future basic."""

import asyncio


async def set_later(fut: asyncio.Future) -> None:
    await asyncio.sleep(0)
    fut.set_result("ok")


async def main() -> None:
    fut: asyncio.Future = asyncio.Future()
    asyncio.create_task(set_later(fut))
    print(await fut)


asyncio.run(main())
