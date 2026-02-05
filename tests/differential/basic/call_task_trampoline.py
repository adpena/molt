"""Purpose: dynamic calls of generator/coroutine/asyncgen functions use task-aware trampolines."""
import asyncio


def gen(a, b):
    yield a + b


async def coro(x):
    return x + 1


async def agen(x):
    yield x + 2


def run_gen():
    f = gen
    g = f(1, 2)
    print(next(g))


async def run_async():
    f = coro
    c = f(4)
    print(await c)
    agf = agen
    ag = agf(5)
    print(await ag.__anext__())


def main():
    run_gen()
    asyncio.run(run_async())


if __name__ == "__main__":
    main()
