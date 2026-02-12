"""Purpose: differential coverage for asyncio.timeout context manager."""

import asyncio


async def main() -> None:
    try:
        async with asyncio.timeout(0.01):
            await asyncio.sleep(0.02)
    except TimeoutError as exc:
        print(type(exc).__name__)


asyncio.run(main())
