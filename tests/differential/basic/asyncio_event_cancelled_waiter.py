"""Purpose: differential coverage for asyncio event cancelled waiter."""

import asyncio


async def waiter(ev: asyncio.Event, label: str) -> str:
    try:
        await ev.wait()
        return f"{label}:set"
    except asyncio.CancelledError:
        return f"{label}:cancelled"


async def main() -> None:
    ev = asyncio.Event()
    first = asyncio.create_task(waiter(ev, "a"))
    second = asyncio.create_task(waiter(ev, "b"))
    await asyncio.sleep(0)
    first.cancel()
    await asyncio.sleep(0)
    ev.set()
    res = await asyncio.gather(first, second)
    print(res)


asyncio.run(main())
