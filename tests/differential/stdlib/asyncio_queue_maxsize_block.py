"""Purpose: differential coverage for asyncio queue maxsize block."""

import asyncio


async def main() -> None:
    q: asyncio.Queue[int] = asyncio.Queue(maxsize=1)
    order: list[str] = []

    async def producer() -> None:
        await q.put(1)
        order.append("put1")
        await q.put(2)
        order.append("put2")

    async def consumer() -> None:
        await asyncio.sleep(0)
        order.append(f"get{await q.get()}")
        await asyncio.sleep(0)
        order.append(f"get{await q.get()}")

    await asyncio.gather(producer(), consumer())
    print(order)


asyncio.run(main())
