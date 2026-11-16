"""A range loop inside a generator or coroutine counts across suspensions:
range() is evaluated once, `continue` advances, `break` and `else` behave as
in any function, and the loop variable keeps its last value."""

import asyncio


def gen_var(n):
    for k in range(n):
        yield k


def gen_steps():
    for k in range(10, 0, -3):
        yield k
    for k in range(2, 12, 4):
        yield k


def gen_flow(n):
    for k in range(n):
        if k % 2:
            continue
        if k > 6:
            break
        yield k
    else:
        yield "no-break"
    yield ("last", k)


def gen_once(n):
    stop = n
    for k in range(stop):
        stop = 0
        yield k


def gen_nested(n):
    for i in range(n):
        for j in range(i):
            yield (i, j)


def gen_big():
    base = 2**64
    for k in range(base, base + 3):
        yield k - base


def gen_empty():
    for k in range(5, 0):
        yield k
    else:
        yield "empty-else"


print(list(gen_var(5)), sum(gen_var(10)))
print(list(gen_steps()))
print(list(gen_flow(20)), list(gen_flow(3)))
print(list(gen_once(4)))
print(list(gen_nested(4)))
print(list(gen_big()), list(gen_empty()))


async def tick():
    await asyncio.sleep(0)


async def coro(n):
    total = 0
    for k in range(n):
        await tick()
        if k == 2:
            continue
        total += k
    return total


print(asyncio.run(coro(6)))
