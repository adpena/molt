"""Purpose: differential coverage for cancelling a sleeping task."""

import asyncio


async def sleeper(log: list[str]) -> None:
    try:
        await asyncio.sleep(1)
        log.append("slept")
    except asyncio.CancelledError:
        log.append("cancelled")
        raise


async def main() -> None:
    log: list[str] = []
    task = asyncio.create_task(sleeper(log))
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        log.append("caught")
    await asyncio.sleep(0)
    for entry in log:
        print(entry)


asyncio.run(main())
