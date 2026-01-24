"""Purpose: differential coverage for TaskGroup task context + name."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def read_var() -> str:
    await asyncio.sleep(0)
    return var.get()


async def main() -> None:
    var.set("main")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "tg")
    tasks: list[asyncio.Task[str]] = []
    async with asyncio.TaskGroup() as tg:
        tasks.append(tg.create_task(read_var(), name="tg-task", context=ctx))
    task = tasks[0]
    print(task.result(), task.get_name(), task.get_context().get(var), var.get())


asyncio.run(main())
