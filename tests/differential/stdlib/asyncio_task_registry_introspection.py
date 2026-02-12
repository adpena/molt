"""Purpose: differential coverage for asyncio task registry introspection."""

import asyncio


async def worker() -> None:
    await asyncio.sleep(0)


async def main() -> None:
    task = asyncio.create_task(worker())
    current = asyncio.current_task()
    in_all_before = task in asyncio.all_tasks()
    await task
    in_all_after = task in asyncio.all_tasks()
    print(in_all_before, in_all_after, current is not None)


asyncio.run(main())
