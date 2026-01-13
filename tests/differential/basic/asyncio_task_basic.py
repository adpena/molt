import asyncio
import contextvars

var = contextvars.ContextVar("var", default="none")


async def child() -> int:
    print("child", var.get())
    print(asyncio.current_task() is not None)
    var.set("child")
    await asyncio.sleep(0)
    print("child2", var.get())
    return 5


async def main() -> None:
    var.set("main")
    task = asyncio.create_task(child())
    var.set("main2")
    res = await task
    print("res", res)
    print("main", var.get())


asyncio.run(main())
