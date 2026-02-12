"""Purpose: differential coverage for asyncio task factory naming."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    seen: list[str | None] = []

    def factory(factory_loop: asyncio.AbstractEventLoop, coro, **kwargs):
        seen.append(kwargs.get("name"))
        return asyncio.Task(coro, loop=factory_loop, **kwargs)

    loop.set_task_factory(factory)

    async def noop() -> None:
        await asyncio.sleep(0)

    task = asyncio.create_task(noop(), name="named-task")
    await task
    loop.set_task_factory(None)
    print(seen, task.get_name())


asyncio.run(main())
