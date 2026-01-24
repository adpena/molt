"""Purpose: differential coverage for asyncio call_soon ordering."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    log: list[str] = []
    loop.call_soon(log.append, "a")
    loop.call_soon(log.append, "b")
    await asyncio.sleep(0)
    print(log)


asyncio.run(main())
