"""Purpose: differential coverage for asyncio condition fairness."""

import asyncio


async def main() -> None:
    cond = asyncio.Condition()
    order: list[str] = []

    async def waiter(label: str) -> None:
        async with cond:
            await cond.wait()
            order.append(label)

    t1 = asyncio.create_task(waiter("a"))
    t2 = asyncio.create_task(waiter("b"))
    await asyncio.sleep(0)

    async with cond:
        cond.notify(1)
    await asyncio.sleep(0)
    async with cond:
        cond.notify(1)

    await asyncio.gather(t1, t2)
    print(order)


asyncio.run(main())
