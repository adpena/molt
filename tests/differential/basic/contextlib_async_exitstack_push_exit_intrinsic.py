import asyncio
import contextlib


events: list[tuple[str, object]] = []


class AsyncManager:
    async def __aexit__(self, exc_type, exc, tb):
        name = exc_type.__name__ if exc_type is not None else None
        events.append(("aexit", name))
        return False


async def async_callback(value):
    events.append(("callback", value))


async def main():
    async with contextlib.AsyncExitStack() as stack:
        manager = AsyncManager()
        pushed_exit = stack.push_async_exit(manager)
        pushed_callback = stack.push_async_callback(async_callback, "ok")
        print("PUSH_RETURNS", pushed_exit is manager, pushed_callback is async_callback)
    print("EVENTS", events)


asyncio.run(main())
