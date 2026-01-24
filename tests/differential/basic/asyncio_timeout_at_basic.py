"""Purpose: differential coverage for asyncio.timeout_at."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 0.05
    try:
        async with asyncio.timeout_at(deadline):
            await asyncio.sleep(0.1)
    except TimeoutError:
        print("timeout")


asyncio.run(main())
