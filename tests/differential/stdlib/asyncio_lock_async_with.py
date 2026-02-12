"""Purpose: differential coverage for asyncio.Lock as async context manager."""

import asyncio


async def main() -> None:
    lock = asyncio.Lock()
    log: list[bool] = []
    async with lock:
        log.append(lock.locked())
    log.append(lock.locked())
    print(log)


asyncio.run(main())
