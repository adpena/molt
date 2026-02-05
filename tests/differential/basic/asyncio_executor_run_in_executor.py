"""Purpose: differential coverage for asyncio run_in_executor."""

import asyncio
from concurrent.futures import ThreadPoolExecutor


def blocking(value: int) -> int:
    return value + 1


async def main() -> None:
    loop = asyncio.get_running_loop()
    result = await loop.run_in_executor(None, blocking, 41)
    print(result)
    with ThreadPoolExecutor(max_workers=1) as executor:
        result = await loop.run_in_executor(executor, blocking, 10)
        print(result)


asyncio.run(main())
