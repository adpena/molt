"""Purpose: differential coverage for asyncio.run pending task cleanup."""

import asyncio


async def sleeper(log: list[str]) -> None:
    try:
        await asyncio.Event().wait()
    finally:
        log.append("cancelled")


async def main(log: list[str]) -> None:
    task = asyncio.create_task(sleeper(log))
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    if task.done():
        log.append("done-early")


log: list[str] = []
asyncio.run(main(log))
print(log)
