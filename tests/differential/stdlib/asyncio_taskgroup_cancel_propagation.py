"""Purpose: differential coverage for asyncio taskgroup cancel propagation."""

import asyncio


async def child(log: list[str]) -> None:
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        log.append("cancelled")
        raise


async def main() -> None:
    log: list[str] = []
    async with asyncio.TaskGroup() as tg:
        task = tg.create_task(child(log))
        await asyncio.sleep(0)
        task.cancel()
    print(log)


asyncio.run(main())
