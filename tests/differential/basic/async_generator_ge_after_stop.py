"""Purpose: differential coverage for athrow(GeneratorExit) after stop."""

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
        await it.athrow(GeneratorExit())
    except Exception as exc:
        print("ge", type(exc).__name__)


asyncio.run(main())
