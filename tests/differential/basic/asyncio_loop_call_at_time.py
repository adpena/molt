"""Purpose: differential coverage for loop.call_at scheduling."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    log: list[str] = []
    when = loop.time() + 0.05
    loop.call_at(when, log.append, "at")
    await asyncio.sleep(0.08)
    print(log)


asyncio.run(main())
