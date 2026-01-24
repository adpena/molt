"""Purpose: differential coverage for generator coroutine states."""

import asyncio
import inspect


def gen():
    yield 1
    yield 2


g = gen()
print(inspect.getgeneratorstate(g))
next(g)
print(inspect.getgeneratorstate(g))
g.close()
print(inspect.getgeneratorstate(g))


async def coro():
    await asyncio.sleep(0)
    return "ok"


c = coro()
print(inspect.getcoroutinestate(c))


async def drive():
    print(await c)
    print(inspect.getcoroutinestate(c))


asyncio.run(drive())
