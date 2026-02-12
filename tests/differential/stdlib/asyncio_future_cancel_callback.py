"""Purpose: differential coverage for Future cancellation callbacks."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    fut: asyncio.Future[None] = loop.create_future()
    log: list[tuple[str, bool]] = []

    def on_done(done: asyncio.Future[None]) -> None:
        log.append(("done", done.cancelled()))

    fut.add_done_callback(on_done)
    fut.cancel()
    await asyncio.sleep(0)
    print(log, fut.cancelled())


asyncio.run(main())
