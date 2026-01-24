"""Purpose: differential coverage for asyncio condition basic."""

import asyncio


async def main() -> None:
    cond = asyncio.Condition()
    ready = {"flag": False}

    async def waiter() -> None:
        async with cond:
            await cond.wait_for(lambda: ready["flag"])
        print("ready")

    async def notifier() -> None:
        await asyncio.sleep(0)
        async with cond:
            ready["flag"] = True
            cond.notify_all()

    await asyncio.gather(asyncio.create_task(waiter()), asyncio.create_task(notifier()))


asyncio.run(main())
