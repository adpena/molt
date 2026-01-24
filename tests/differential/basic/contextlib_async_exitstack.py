"""Purpose: differential coverage for contextlib async exitstack."""

import asyncio
import contextlib


log = []


@contextlib.asynccontextmanager
async def ctx(name: str):
    log.append(f"enter:{name}")
    try:
        yield name
    finally:
        log.append(f"exit:{name}")


async def main() -> None:
    async with contextlib.AsyncExitStack() as stack:
        a = await stack.enter_async_context(ctx("a"))
        b = await stack.enter_async_context(ctx("b"))
        print(a, b)
    print(log)


asyncio.run(main())
