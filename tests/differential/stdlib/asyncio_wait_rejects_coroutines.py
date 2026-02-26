"""Purpose: differential coverage for asyncio.wait coroutine rejection."""

import asyncio


async def worker() -> int:
    await asyncio.sleep(0)
    return 1


async def main() -> None:
    coro = worker()
    try:
        await asyncio.wait({coro})
    except Exception as exc:
        print(type(exc).__name__)
        print(str(exc))
    finally:
        coro.close()


asyncio.run(main())
