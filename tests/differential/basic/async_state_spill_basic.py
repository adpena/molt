"""Purpose: ensure async spill/restore across state_label."""
import asyncio


async def work(x: int) -> int:
    y = x * 2
    await asyncio.sleep(0)
    return y + 3


async def main() -> int:
    return await work(5)


print(asyncio.run(main()))
