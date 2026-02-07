import asyncio


async def _main() -> None:
    loop = asyncio.get_running_loop()
    events: list[str] = []

    def _first() -> None:
        events.append("first")
        loop.call_soon(events.append, "third")

    loop.call_soon(_first)
    loop.call_soon(events.append, "second")
    await asyncio.sleep(0.0)
    await asyncio.sleep(0.0)
    print(events)


asyncio.run(_main())
