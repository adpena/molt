"""Purpose: differential coverage for asyncio run_in_executor."""

import asyncio


def blocking(value: int) -> int:
    return value + 1


async def main() -> None:
    loop = asyncio.get_running_loop()
    result = await loop.run_in_executor(None, blocking, 41)
    print(result)


asyncio.run(main())
