"""Purpose: differential coverage for asyncio run_in_executor."""

import asyncio
from concurrent.futures import ThreadPoolExecutor
import threading


def blocking(value: int) -> int:
    return value + 1


async def main() -> None:
    loop = asyncio.get_running_loop()
    result = await loop.run_in_executor(None, blocking, 41)
    print(result)
    with ThreadPoolExecutor(max_workers=1) as executor:
        result = await loop.run_in_executor(executor, blocking, 10)
        print(result)
    error = ValueError("executor failure")

    def fail():
        raise error

    try:
        await loop.run_in_executor(None, fail)
    except ValueError as caught:
        print("exception identity", caught is error)

    released = threading.Event()
    finished = threading.Event()

    def pending():
        released.wait()
        finished.set()

    loop.run_in_executor(None, pending)
    loop.call_soon(released.set)
    await loop.shutdown_default_executor()
    print("shutdown drained", finished.is_set())
    try:
        loop.run_in_executor(None, blocking, 1)
    except RuntimeError as caught:
        print(type(caught).__name__, str(caught))


asyncio.run(main())
