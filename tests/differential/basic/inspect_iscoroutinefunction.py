"""Purpose: differential coverage for inspect iscoroutinefunction."""

import asyncio
import inspect


async def coro():
    return 1


print(inspect.iscoroutinefunction(coro))
print(inspect.iscoroutinefunction(asyncio.sleep))
