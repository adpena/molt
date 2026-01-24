"""Purpose: differential coverage for async generator completion edges."""

import asyncio


async def agen():
    yield 1


async def main():
    it = agen()
    first = await it.__anext__()
    print("first", first)
    try:
        await it.asend(10)
    except Exception as exc:
        print("asend_after", type(exc).__name__)
    try:
        await it.athrow(GeneratorExit())
    except Exception as exc:
        print("athrow_ge", type(exc).__name__)


asyncio.run(main())
