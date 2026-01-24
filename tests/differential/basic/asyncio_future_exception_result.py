"""Purpose: differential coverage for Future exception/result access."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    fut: asyncio.Future[None] = loop.create_future()
    fut.set_exception(ValueError("boom"))
    exc = fut.exception()
    print(type(exc).__name__, str(exc))
    try:
        fut.result()
    except Exception as err:
        print(type(err).__name__)
    try:
        await fut
    except Exception as err:
        print(type(err).__name__)


asyncio.run(main())
