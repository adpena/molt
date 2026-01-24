"""Purpose: differential coverage for asyncio queue basic."""

import asyncio


async def main() -> None:
    queue = asyncio.Queue(maxsize=1)
    order: list[str] = []
    start = asyncio.Event()

    async def consumer() -> None:
        start.set()
        order.append(await queue.get())
        order.append(await queue.get())

    async def producer() -> None:
        await start.wait()
        await queue.put("a")
        await queue.put("b")

    await asyncio.gather(consumer(), producer())
    print(order)


asyncio.run(main())
