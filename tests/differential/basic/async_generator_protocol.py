"""Purpose: differential coverage for async generator asend/athrow/aclose."""

import asyncio


async def agen():
    try:
        value = yield 1
        yield value
    finally:
        yield "final"


async def main():
    it = agen()
    first = await it.__anext__()
    print("first", first)
    second = await it.asend(10)
    print("second", second)
    try:
        await it.athrow(ValueError("boom"))
    except Exception as exc:
        print("athrow", type(exc).__name__)
    try:
        final = await it.aclose()
        print("aclose", final)
    except Exception as exc:
        print("aclose_err", type(exc).__name__)


asyncio.run(main())
