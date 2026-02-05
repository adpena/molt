"""Purpose: differential coverage for asyncio cancellation semantics."""

import asyncio


async def main() -> None:
    async def worker() -> None:
        try:
            await asyncio.sleep(1)
        except asyncio.CancelledError:
            print("worker-cancelled")
            raise

    task = asyncio.create_task(worker())
    await asyncio.sleep(0)
    print("task-done-before", task.done())
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        print("cancelled-error")
    print("task-cancelled", task.cancelled())


asyncio.run(main())
