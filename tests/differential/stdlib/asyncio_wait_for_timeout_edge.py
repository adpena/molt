"""Purpose: differential coverage for asyncio wait for timeout edge."""

import asyncio


async def main() -> None:
    async def sleeper() -> None:
        await asyncio.sleep(1)

    try:
        await asyncio.wait_for(sleeper(), timeout=0.0)
    except Exception as exc:
        print(type(exc).__name__)


asyncio.run(main())
