"""Purpose: differential coverage for asyncio wait for basic."""

import asyncio


async def fast() -> str:
    await asyncio.sleep(0)
    return "fast"


async def slow() -> str:
    await asyncio.sleep(1)
    return "slow"


async def main() -> None:
    print(await asyncio.wait_for(fast(), timeout=0.1))
    try:
        await asyncio.wait_for(slow(), timeout=0)
    except TimeoutError:
        print("timeout")
    print(await asyncio.wait_for(fast(), timeout=None))


asyncio.run(main())
