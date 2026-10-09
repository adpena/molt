"""Purpose: values a loop reads must survive a resume inside the loop body.

Every comprehension and loop below suspends inside its body. A resume enters
the poll at the suspension's state label, so any value the lowering computed
once before the loop (iteration constants, guards, iterators) must come back
from the frame on every later iteration. A lost value shows up as a wrong
element, a skipped iteration or a TypeError.
"""

import asyncio


async def agen(n):
    for i in range(n):
        await asyncio.sleep(0)
        yield i


async def inc(x):
    await asyncio.sleep(0)
    return x + 1


async def list_forms():
    nested = [[await inc(y) async for y in agen(3)] for _ in range(2)]
    flat = [await inc(i) for i in range(4)]
    pairs = [(i, await inc(j)) for i in range(2) for j in range(2)]
    filtered = [await inc(i) for i in range(6) if i % 2]
    deep = [[[await inc(k) for k in range(2)] for _ in range(2)] for _ in range(2)]
    return nested, flat, pairs, filtered, deep


async def other_comprehensions():
    squares = {await inc(i) * 2 for i in range(4)}
    mapping = {i: await inc(i) for i in range(3)}
    mixed = [y async for x in agen(3) for y in (x, await inc(x))]
    return sorted(squares), mapping, mixed


async def statement_loops(items):
    seen = []
    for index, item in enumerate(items):
        seen.append((index, await inc(item)))
    else:
        seen.append("for-else")
    total = 0
    count = 0
    while count < 3:
        total += await inc(count)
        count += 1
    async for value in agen(3):
        for inner in range(2):
            seen.append((value, inner, await inc(inner)))
    return seen, total


async def async_generator_with_comprehension():
    for round_number in range(2):
        yield [await inc(round_number * 10 + i) for i in range(3)]


def sync_generator(items):
    for index, item in enumerate(items):
        yield [item * value for value in range(index + 1)]


async def main():
    print(await list_forms())
    print(await other_comprehensions())
    print(await statement_loops([5, 6, 7]))
    print([batch async for batch in async_generator_with_comprehension()])
    print(list(sync_generator([1, 2, 3])))


asyncio.run(main())
