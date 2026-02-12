"""Purpose: differential coverage for asyncio task current identity."""

import asyncio


async def child(label: str) -> None:
    task = asyncio.current_task()
    print(label, task is not None)


async def main() -> None:
    t = asyncio.create_task(child("c"))
    await t
    print("main", asyncio.current_task() is not None)


asyncio.run(main())
