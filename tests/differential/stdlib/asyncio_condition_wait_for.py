"""Purpose: differential coverage for asyncio.Condition.wait_for."""

import asyncio


async def main() -> None:
    condition = asyncio.Condition()
    state = {"ready": False}

    async def waiter() -> None:
        async with condition:
            ok = await condition.wait_for(lambda: state["ready"])
            print("waiter", ok)

    async def setter() -> None:
        await asyncio.sleep(0)
        async with condition:
            state["ready"] = True
            condition.notify()

    await asyncio.gather(waiter(), setter())


asyncio.run(main())
