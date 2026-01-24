"""Purpose: differential coverage for async generator reentrancy errors."""

import asyncio


async def agen():
    await asyncio.sleep(0)
    yield 1


async def main() -> None:
    it = agen()
    task = asyncio.create_task(it.__anext__())
    await asyncio.sleep(0)
    try:
        await it.__anext__()
    except Exception as exc:
        print("reenter", type(exc).__name__, str(exc))
    await task


asyncio.run(main())
