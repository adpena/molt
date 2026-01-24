"""Purpose: differential coverage for asyncio wait return all."""

import asyncio


async def worker(label: str, delay: float) -> str:
    await asyncio.sleep(delay)
    return label


async def main() -> None:
    a = asyncio.create_task(worker("a", 0))
    b = asyncio.create_task(worker("b", 0.01))
    done, pending = await asyncio.wait({a, b}, return_when=asyncio.ALL_COMPLETED)
    print(sorted(task.result() for task in done), len(pending))


asyncio.run(main())
