"""Purpose: differential coverage for asyncio.wait FIRST_COMPLETED."""

import asyncio


async def fast() -> str:
    await asyncio.sleep(0)
    return "fast"


async def slow() -> str:
    await asyncio.sleep(0.05)
    return "slow"


async def main() -> None:
    t1 = asyncio.create_task(fast())
    t2 = asyncio.create_task(slow())
    done, pending = await asyncio.wait(
        {t1, t2}, return_when=asyncio.FIRST_COMPLETED
    )
    results = sorted(task.result() for task in done)
    for task in pending:
        task.cancel()
    await asyncio.gather(*pending, return_exceptions=True)
    print(results, len(pending))


asyncio.run(main())
