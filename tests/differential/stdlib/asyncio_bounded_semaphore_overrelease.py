"""Purpose: differential coverage for BoundedSemaphore over-release."""

import asyncio


async def main() -> None:
    sem = asyncio.BoundedSemaphore(1)
    await sem.acquire()
    sem.release()
    try:
        sem.release()
    except ValueError as exc:
        print(type(exc).__name__)


asyncio.run(main())
