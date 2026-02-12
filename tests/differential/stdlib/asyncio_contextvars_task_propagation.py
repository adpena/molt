"""Purpose: differential coverage for contextvars propagation into tasks."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def main() -> None:
    var.set("task-value")

    async def read_var() -> str:
        return var.get()

    task = asyncio.create_task(read_var())
    print(await task)


asyncio.run(main())
