"""Purpose: differential coverage for asend(None) after close/stop."""

import asyncio


async def agen():
    yield 1


async def main():
    it = agen()
    await it.__anext__()
    await it.aclose()
    try:
        await it.asend(None)
    except Exception as exc:
        print("after_close", type(exc).__name__)

    it = agen()
    await it.__anext__()
    try:
        await it.__anext__()
    except Exception as exc:
        print("stop", type(exc).__name__)
    try:
        await it.asend(None)
    except Exception as exc:
        print("after_stop", type(exc).__name__)


asyncio.run(main())
