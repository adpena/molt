import asyncio

x = 1


async def mutate(new_val: int, ret: int) -> int:
    global x
    await asyncio.sleep(0)
    x = new_val
    return ret


def add(a: int, b: int) -> int:
    return a + b


async def main() -> tuple[int, int, bool, bool, int, int]:
    global x
    x = 1
    a = x + await mutate(10, 5)
    x = 1
    b = add(x, await mutate(10, 5))
    x = 1
    c = x < await mutate(10, 5)
    x = 1
    d = 0 < await mutate(5, 3) < 4
    x = 0
    e = x or await mutate(1, 7)
    return a, b, c, d, e, x


print(asyncio.run(main()))
