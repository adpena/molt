"""Purpose: differential coverage for explicit task context injection."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def read_var() -> str:
    return var.get()


async def main() -> None:
    var.set("main")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "override")
    task = asyncio.create_task(read_var(), name="ctx-task", context=ctx)
    value = await task
    print(value, task.get_context().get(var), var.get(), task.get_name())


asyncio.run(main())
