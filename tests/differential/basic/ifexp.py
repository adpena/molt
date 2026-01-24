"""Purpose: differential coverage for ifexp."""

import asyncio


def pick(a, b, cond):
    return a if cond else b


print(pick(1, 2, True))
print(pick(1, 2, False))

hits = []


def t():
    hits.append("t")
    return 1


def f():
    hits.append("f")
    return 2


print(t() if True else f())
print(hits)

hits = []
print(t() if False else f())
print(hits)


async def slow():
    await asyncio.sleep(0)
    return 2


async def choose(cond):
    return 1 if cond else await slow()


print(asyncio.run(choose(True)))
print(asyncio.run(choose(False)))
