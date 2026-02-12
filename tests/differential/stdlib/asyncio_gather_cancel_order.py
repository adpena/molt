"""Purpose: differential coverage for asyncio gather cancel order."""

import asyncio


async def child(name: str, log: list[str]) -> None:
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        log.append(f"cancelled:{name}")
        raise


async def main() -> None:
    log: list[str] = []
    t1 = asyncio.create_task(child("a", log))
    t2 = asyncio.create_task(child("b", log))
    await asyncio.sleep(0)
    t1.cancel()
    try:
        await asyncio.gather(t1, t2)
    except Exception as exc:
        print(type(exc).__name__)
    print(sorted(log))


asyncio.run(main())
