"""Purpose: differential coverage for async generator asend after close."""

import asyncio


async def agen():
    yield 1


async def main():
    it = agen()
    await it.__anext__()
    await it.aclose()
    try:
        await it.asend(10)
    except Exception as exc:
        print("asend_after_close", type(exc).__name__)


asyncio.run(main())
