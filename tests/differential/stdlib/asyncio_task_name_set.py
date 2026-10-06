"""Purpose: differential coverage for task name setters."""

import asyncio


async def main() -> None:
    async def noop() -> None:
        await asyncio.sleep(0)

    task = asyncio.create_task(noop(), name="first")
    task.set_name("second")
    await task
    print(task.get_name())

    for name in ("", 0, False):
        task = asyncio.create_task(noop(), name=name)
        print("name", repr(task.get_name()), type(task.get_name()).__name__)
        task.set_name(name)
        print("renamed", repr(task.get_name()), type(task.get_name()).__name__)
        for method, value in ((task.set_result, 7), (task.set_exception, ValueError("external"))):
            try:
                method(value)
            except RuntimeError as error:
                print("external-completion", str(error), task.done())
        await task


asyncio.run(main())
