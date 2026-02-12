"""Purpose: differential coverage for asyncio cancel try finally."""

import asyncio


async def worker(log: list[str]) -> None:
    try:
        await asyncio.sleep(1)
    finally:
        log.append("finally")


async def main() -> None:
    log: list[str] = []
    task = asyncio.create_task(worker(log))
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except Exception as exc:
        print(type(exc).__name__)
    print(log)


asyncio.run(main())
