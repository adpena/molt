# MOLT_ENV: MOLT_ASYNC_HANG_PROBE=5

import asyncio


async def main() -> None:
    total = 0
    for i in range(8):
        await asyncio.sleep(0)
        total += i
    print(total)


asyncio.run(main())
