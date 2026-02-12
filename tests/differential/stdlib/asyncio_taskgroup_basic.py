"""Purpose: differential coverage for asyncio taskgroup basic."""

import asyncio


async def worker(val: int, out: list[int]) -> None:
    await asyncio.sleep(0)
    out.append(val)


async def main() -> None:
    results: list[int] = []
    async with asyncio.TaskGroup() as group:
        group.create_task(worker(2, results))
        group.create_task(worker(1, results))
    print(sorted(results))


asyncio.run(main())
