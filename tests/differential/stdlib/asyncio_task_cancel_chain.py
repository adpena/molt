"""Purpose: differential coverage for asyncio task cancel chain."""

import asyncio


async def child() -> None:
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        raise


async def parent() -> None:
    task = asyncio.create_task(child())
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        print("child_cancelled")


asyncio.run(parent())
