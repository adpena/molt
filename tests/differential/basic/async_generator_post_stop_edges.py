"""Purpose: differential coverage for async generator post-StopAsyncIteration edges."""

import asyncio


async def agen():
    if False:
        yield 1


async def main():
    it = agen()
    try:
        await it.__anext__()
    except Exception as exc:
        print("first", type(exc).__name__)
    try:
        await it.asend(None)
    except Exception as exc:
        print("asend", type(exc).__name__)
    try:
        await it.athrow(ValueError("boom"))
    except Exception as exc:
        print("athrow", type(exc).__name__)
    try:
        await it.aclose()
        print("aclose", "ok")
    except Exception as exc:
        print("aclose", type(exc).__name__)


asyncio.run(main())
