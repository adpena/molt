import asyncio


def nonlocal_basic() -> int:
    x = 1

    def inner() -> None:
        nonlocal x
        x += 2

    inner()
    return x


def nonlocal_late_binding() -> int:
    def inner() -> None:
        nonlocal x
        x = 5

    x = 1
    inner()
    return x


def class_method_closure() -> int:
    x = 7

    class C:
        def f(self) -> int:
            return x + 1

    return C().f()


async def async_nonlocal() -> int:
    x = 1

    async def bump() -> None:
        nonlocal x
        x += 3

    await bump()
    return x


print(nonlocal_basic())
print(nonlocal_late_binding())
print(class_method_closure())
print(asyncio.run(async_nonlocal()))
