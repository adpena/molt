"""Purpose: differential coverage for PEP 572 walrus operator edges."""

import asyncio


async def await_value() -> int:
    await asyncio.sleep(0)
    return 5


async def main() -> None:
    value = (x := 1)
    print(x, value)

    get_value = lambda: (y := 3)
    print(get_value())

    total = 0
    it = iter([1, 2, 3])
    while (n := next(it, None)) is not None:
        total += n
    print(total)

    result = (z := await await_value())
    print(z, result)


asyncio.run(main())
