"""Purpose: differential coverage for asyncio.run shutting down async generators."""

import asyncio


async def agen(log: list[str]) -> int:
    try:
        yield 1
        await asyncio.sleep(0)
    finally:
        log.append("closed")


async def main(log: list[str]) -> None:
    generator = agen(log)
    await generator.__anext__()


log: list[str] = []
asyncio.run(main(log))
print(log)
