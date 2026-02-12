"""Purpose: differential coverage for asyncio wait first exception timeout."""

import asyncio


async def boom(delay: float) -> None:
    await asyncio.sleep(delay)
    raise ValueError("boom")


async def sleeper(delay: float) -> None:
    await asyncio.sleep(delay)


async def main() -> None:
    # Exception before timeout.
    t1 = asyncio.create_task(boom(0))
    t2 = asyncio.create_task(sleeper(0.1))
    done, pending = await asyncio.wait(
        {t1, t2},
        timeout=0.5,
        return_when=asyncio.FIRST_EXCEPTION,
    )
    excs = [type(task.exception()).__name__ for task in done if task.exception()]
    print("case1", sorted(excs), len(pending))
    for task in pending:
        task.cancel()
    await asyncio.gather(*pending, return_exceptions=True)

    # Timeout before exception.
    t3 = asyncio.create_task(boom(0.2))
    t4 = asyncio.create_task(sleeper(0.2))
    done2, pending2 = await asyncio.wait(
        {t3, t4},
        timeout=0.01,
        return_when=asyncio.FIRST_EXCEPTION,
    )
    print("case2", len(done2), len(pending2))
    for task in pending2:
        task.cancel()
    await asyncio.gather(*pending2, return_exceptions=True)


asyncio.run(main())
