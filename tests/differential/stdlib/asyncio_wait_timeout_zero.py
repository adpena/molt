"""Purpose: differential coverage for asyncio wait timeout zero."""

import asyncio


async def main() -> None:
    async def sleeper() -> None:
        await asyncio.sleep(0)

    t = asyncio.create_task(sleeper())
    done, pending = await asyncio.wait({t}, timeout=0.0)
    print(len(done), len(pending))
    for task in pending:
        await task


asyncio.run(main())
