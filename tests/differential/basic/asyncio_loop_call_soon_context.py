"""Purpose: differential coverage for call_soon context injection."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def main() -> None:
    loop = asyncio.get_running_loop()
    var.set("main")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "override")
    fut: asyncio.Future[str] = loop.create_future()

    def callback() -> None:
        fut.set_result(var.get())

    loop.call_soon(callback, context=ctx)
    result = await fut
    print(result, var.get())


asyncio.run(main())
