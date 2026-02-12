"""Purpose: differential coverage for asyncio gather cancel sibling."""

import asyncio


async def slow() -> int:
    try:
        await asyncio.sleep(1)
        return 1
    except asyncio.CancelledError:
        print("slow_cancelled")
        raise


async def boom() -> int:
    await asyncio.sleep(0)
    raise ValueError("boom")


async def main() -> None:
    try:
        await asyncio.gather(slow(), boom())
    except Exception as exc:
        print(type(exc).__name__)
    await asyncio.sleep(0)


asyncio.run(main())
