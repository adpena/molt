"""Purpose: differential coverage for contextvars propagation into callbacks."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def main() -> None:
    loop = asyncio.get_running_loop()
    var.set("callback-value")
    fut: asyncio.Future[str] = loop.create_future()

    def callback() -> None:
        fut.set_result(var.get())

    loop.call_soon(callback)
    print(await fut)


asyncio.run(main())
