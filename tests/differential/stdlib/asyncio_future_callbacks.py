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

    print(events)

    for outcome in ("result", "exception", "cancel"):
        pending = asyncio.get_running_loop().create_future()
        order = []
        pending.add_done_callback(lambda done: order.append("before"))

        async def wait(label):
            try:
                await pending
            except (ValueError, asyncio.CancelledError):
                pass
            order.append(label)

        first = asyncio.create_task(wait("first-waiter"))
        await asyncio.sleep(0)
        pending.add_done_callback(lambda done: order.append("between"))
        second = asyncio.create_task(wait("second-waiter"))
        await asyncio.sleep(0)
        pending.add_done_callback(lambda done: order.append("after"))
        if outcome == "result":
            pending.set_result(42)
        elif outcome == "exception":
            pending.set_exception(ValueError("original"))
        else:
            pending.cancel("cancelled")
        await first
        await second
        print(outcome, order)


asyncio.run(main())
