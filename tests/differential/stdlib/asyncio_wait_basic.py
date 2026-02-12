"""Purpose: differential coverage for asyncio wait basic."""

import asyncio


async def sleeper(label: str, delay: float) -> str:
    await asyncio.sleep(delay)
    return label


async def main() -> None:
    a = asyncio.create_task(sleeper("a", 0))
    b = asyncio.create_task(sleeper("b", 0.1))
    done, pending = await asyncio.wait({a, b}, return_when=asyncio.FIRST_COMPLETED)
    done_labels = sorted(task.result() for task in done)
    pending_count = len(pending)
    print(done_labels, pending_count)
    for task in pending:
        await task


asyncio.run(main())
