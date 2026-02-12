"""Purpose: differential coverage for asyncio.run inside running loop."""

import asyncio


async def main() -> None:
    try:
        asyncio.run(asyncio.sleep(0))
    except RuntimeError as exc:
        print(type(exc).__name__)


asyncio.run(main())
