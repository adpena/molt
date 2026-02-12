"""Purpose: differential coverage for task name setters."""

import asyncio


async def main() -> None:
    async def noop() -> None:
        await asyncio.sleep(0)

    task = asyncio.create_task(noop(), name="first")
    task.set_name("second")
    await task
    print(task.get_name())


asyncio.run(main())
