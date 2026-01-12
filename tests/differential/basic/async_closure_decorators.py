import asyncio


def make_adder(base: int):
    async def add(value: int) -> int:
        await asyncio.sleep(0)
        return base + value

    return add


def async_deco(fn):
    async def wrapper(*args, **kwargs):
        await asyncio.sleep(0)
        return await fn(*args, **kwargs)

    return wrapper


@async_deco
async def greet(name: str) -> str:
    await asyncio.sleep(0)
    return "hi " + name


async def main() -> tuple[int, str, str]:
    add5 = make_adder(5)
    res1 = await add5(7)
    res2 = await greet("molt")
    res3 = await make_adder(10)(-3)
    return (res1, res2, str(res3))


print(asyncio.run(main()))
