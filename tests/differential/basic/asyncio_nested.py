import asyncio as aio
from asyncio import run, sleep


async def inner(val: int) -> int:
    await sleep(0)
    return val + 1


async def middle(val: int) -> int:
    out = await inner(val)
    return out * 2


async def outer() -> int:
    first = await middle(3)
    second = await inner(4)
    return first + second


print(aio.run(outer()))
print(run(outer()))
