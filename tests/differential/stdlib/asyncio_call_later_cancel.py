"""Purpose: differential coverage for asyncio call_later cancellation."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    log: list[str] = []
    handle = loop.call_later(0.05, log.append, "late")
    handle.cancel()
    await asyncio.sleep(0.08)
    print(handle.cancelled(), log)


asyncio.run(main())
