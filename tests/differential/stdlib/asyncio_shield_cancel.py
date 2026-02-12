"""Purpose: differential coverage for asyncio shield cancel."""

import asyncio


async def work() -> str:
    await asyncio.sleep(0)
    return "ok"


async def cancel_soon(task: asyncio.Task) -> None:
    await asyncio.sleep(0)
    task.cancel()


async def main() -> None:
    inner = asyncio.create_task(work())
    outer = asyncio.current_task()
    asyncio.create_task(cancel_soon(outer))
    try:
        await asyncio.shield(inner)
    except asyncio.CancelledError:
        print("outer_cancelled")
    print(await inner)


asyncio.run(main())
