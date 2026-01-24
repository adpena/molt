"""Purpose: differential coverage for athrow after StopAsyncIteration."""

import asyncio


async def agen():
    if False:
        yield 1


async def main():
    it = agen()
    try:
        await it.__anext__()
    except Exception:
        pass
    try:
        await it.athrow(ValueError("boom"))
    except Exception as exc:
        print("athrow_after_stop", type(exc).__name__)


asyncio.run(main())
