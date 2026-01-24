"""Purpose: differential coverage for async generator completion protocol edges."""

import asyncio


async def agen():
    yield 1


async def main():
    it = agen()
    first = await it.__anext__()
    print("first", first)
    try:
        await it.__anext__()
    except Exception as exc:
        print("complete", type(exc).__name__)
    try:
        await it.asend(None)
    except Exception as exc:
        print("asend", type(exc).__name__)
    try:
        await it.athrow(GeneratorExit())
    except Exception as exc:
        print("athrow_ge", type(exc).__name__)


asyncio.run(main())
