"""Purpose: differential coverage for asyncio shim."""

import asyncio as aio
from asyncio import run, sleep


async def main() -> int:
    print("tick")
    await sleep(0)
    print("tock")
    return 7


print(aio.run(main()))
print(run(main()))
