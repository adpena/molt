"""Purpose: differential coverage for async generator athrow after close."""

import asyncio


async def agen():
    yield 1


async def main():
    it = agen()
    await it.__anext__()
    await it.aclose()
    try:
        await it.athrow(ValueError("boom"))
    except Exception as exc:
        print("athrow_after_close", type(exc).__name__)


asyncio.run(main())
