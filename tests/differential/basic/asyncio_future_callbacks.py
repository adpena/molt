"""Purpose: differential coverage for asyncio future callbacks."""

import asyncio


async def main() -> None:
    fut: asyncio.Future[int] = asyncio.Future()
    events: list[str] = []

    def on_done(label: str):
        def inner(_fut: asyncio.Future) -> None:
            events.append(label)

        return inner

    fut.add_done_callback(on_done("early"))

    async def setter() -> None:
        await asyncio.sleep(0)
        fut.set_result(42)

    asyncio.create_task(setter())
    await fut

    fut.add_done_callback(on_done("late"))
    await asyncio.sleep(0)

    print(sorted(events))


asyncio.run(main())
