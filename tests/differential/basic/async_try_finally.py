"""Purpose: differential coverage for async try finally."""

import asyncio


async def worker() -> list:
    out = []
    try:
        await asyncio.sleep(0)
        out.append("work")
    finally:
        out.append("cleanup")
    return out


print(asyncio.run(worker()))
