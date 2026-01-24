"""Purpose: differential coverage for async generator finalization."""

import asyncio


async def agen(log: list[str]):
    try:
        yield 1
    finally:
        log.append("finalized")


async def main() -> None:
    log: list[str] = []
    it = agen(log)
    val = await it.__anext__()
    print(val)
    await it.aclose()
    print(log)


asyncio.run(main())
