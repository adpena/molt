"""Purpose: differential coverage for inspect isawaitable."""

import asyncio
import inspect


async def coro():
    return 1


c = coro()
print(inspect.isawaitable(c))

asyncio.run(c)
