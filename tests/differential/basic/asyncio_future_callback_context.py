"""Purpose: differential coverage for Future callback context capture."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def main() -> None:
    loop = asyncio.get_running_loop()
    var.set("main")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "callback")
    fut: asyncio.Future[int] = loop.create_future()
    seen: list[str] = []

    def on_done(_fut: asyncio.Future[int]) -> None:
        seen.append(var.get())

    fut.add_done_callback(on_done, context=ctx)
    fut.set_result(1)
    await asyncio.sleep(0)
    print(seen, var.get())


asyncio.run(main())
