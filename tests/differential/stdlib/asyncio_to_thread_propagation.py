"""Purpose: differential coverage for asyncio.to_thread context propagation."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


def read_var() -> str:
    return var.get()


async def main() -> None:
    var.set("value")
    result = await asyncio.to_thread(read_var)
    print(result)


asyncio.run(main())
