"""Purpose: differential coverage for Future.cancel(message) propagation."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    fut: asyncio.Future[None] = loop.create_future()
    fut.cancel("future-msg")
    try:
        await fut
    except asyncio.CancelledError as exc:
        print("await", exc.args)
    try:
        fut.result()
    except asyncio.CancelledError as exc:
        print("result", exc.args)
    print(fut.cancelled(), fut.done())


asyncio.run(main())
