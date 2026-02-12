"""Purpose: differential coverage for asyncio ensure future passthrough."""

import asyncio


async def main() -> None:
    fut = asyncio.Future()
    got = asyncio.ensure_future(fut)
    print(fut is got)


asyncio.run(main())
