"""Purpose: differential coverage for nested async comprehensions."""

import asyncio


async def agen():
    for i in range(4):
        yield i


async def inc(x: int) -> int:
    await asyncio.sleep(0)
    return x + 1


async def main():
    result = [await inc(x) async for x in agen() if x % 2 == 0]
    nested = [[await inc(y) async for y in agen()] for _ in range(2)]
    mixed = [y async for x in agen() for y in (x, x + 10)]
    return result, nested, mixed


print(asyncio.run(main()))
