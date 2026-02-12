"""Purpose: differential coverage for asyncio event basic."""

import asyncio


async def setter(ev: asyncio.Event) -> None:
    await asyncio.sleep(0)
    ev.set()


async def main() -> None:
    ev = asyncio.Event()
    asyncio.create_task(setter(ev))
    print(await ev.wait())
    ev.clear()
    ev.set()
    print(await ev.wait())


asyncio.run(main())
