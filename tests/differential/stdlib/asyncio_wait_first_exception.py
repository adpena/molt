"""Purpose: differential coverage for asyncio wait first exception."""

import asyncio


async def boom() -> None:
    await asyncio.sleep(0)
    raise ValueError("boom")


async def slow() -> str:
    await asyncio.sleep(0.05)
    return "ok"


async def main() -> None:
    t1 = asyncio.create_task(boom())
    t2 = asyncio.create_task(slow())
    done, pending = await asyncio.wait(
        {t1, t2},
        return_when=asyncio.FIRST_EXCEPTION,
    )
    exc_names: list[str] = []
    for task in done:
        exc = task.exception()
        if exc is not None:
            exc_names.append(type(exc).__name__)
    print(sorted(exc_names), len(pending))
    for task in pending:
        await task


asyncio.run(main())
