"""Purpose: differential coverage for contextlib asynccontextmanager."""

import asyncio
import contextlib


log = []


@contextlib.asynccontextmanager
async def ctx():
    log.append("enter")
    try:
        yield "value"
    finally:
        log.append("exit")


async def main() -> None:
    async with ctx() as val:
        print(val)
    print(log)


asyncio.run(main())
