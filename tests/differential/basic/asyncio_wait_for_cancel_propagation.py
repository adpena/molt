"""Purpose: differential coverage for asyncio wait for cancel propagation."""

import asyncio


async def slow() -> str:
    try:
        await asyncio.sleep(1)
        return "done"
    except asyncio.CancelledError:
        print("cancelled")
        raise


async def main() -> None:
    task = asyncio.create_task(slow())
    try:
        await asyncio.wait_for(task, timeout=0)
    except asyncio.TimeoutError:
        print("timeout")
    print(task.cancelled(), task.done())
    try:
        await task
    except asyncio.CancelledError:
        print("caught")


asyncio.run(main())
