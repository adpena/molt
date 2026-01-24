"""Purpose: differential coverage for nested asyncio.timeout deadlines."""

import asyncio


async def main() -> None:
    log: list[str] = []
    try:
        async with asyncio.timeout(0.05):
            try:
                async with asyncio.timeout(0.01):
                    await asyncio.sleep(0.02)
            except TimeoutError:
                log.append("inner")
            await asyncio.sleep(0)
            log.append("outer-ok")
    except TimeoutError:
        log.append("outer")
    print(log)


asyncio.run(main())
