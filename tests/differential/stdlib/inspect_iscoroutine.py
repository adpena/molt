"""Purpose: verify inspect/asyncio coroutine detection stays intrinsic-backed."""

import asyncio
import inspect


async def sample() -> int:
    return 1


class Awaitable:
    def __await__(self):
        if False:
            yield None
        return 42


coro = sample()
try:
    print("inspect-coro", inspect.iscoroutine(coro))
    print("asyncio-coro", asyncio.iscoroutine(coro))
    obj = Awaitable()
    print("inspect-awaitable", inspect.iscoroutine(obj))
    print("asyncio-awaitable", asyncio.iscoroutine(obj))
    print("iscoro-func", asyncio.iscoroutinefunction(sample))
finally:
    coro.close()
