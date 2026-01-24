"""Purpose: differential coverage for async generator close semantics."""

import asyncio


async def agen():
    try:
        yield 1
    finally:
        yield "final"


async def main():
    it = agen()
    first = await it.__anext__()
    print("first", first)
    try:
        await it.aclose()
        print("closed", "ok")
    except Exception as exc:
        print("closed", type(exc).__name__)
    try:
        await it.__anext__()
    except Exception as exc:
        print("after_close", type(exc).__name__)


asyncio.run(main())
