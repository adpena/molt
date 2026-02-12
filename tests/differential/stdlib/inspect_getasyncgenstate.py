"""Purpose: differential coverage for inspect getasyncgenstate."""

import inspect
import asyncio


async def agen():
    yield 1


ag = agen()
print(inspect.getasyncgenstate(ag))


async def main() -> None:
    await ag.__anext__()
    print(inspect.getasyncgenstate(ag))
    await ag.aclose()
    print(inspect.getasyncgenstate(ag))


asyncio.run(main())
