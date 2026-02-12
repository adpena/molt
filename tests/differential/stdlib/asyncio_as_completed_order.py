"""Purpose: differential coverage for asyncio.as_completed ordering."""

import asyncio


async def worker(label: str, delay: float) -> str:
    await asyncio.sleep(delay)
    return label


async def main() -> None:
    tasks = [
        asyncio.create_task(worker("a", 0.03)),
        asyncio.create_task(worker("b", 0.01)),
        asyncio.create_task(worker("c", 0.02)),
    ]
    results: list[str] = []
    for fut in asyncio.as_completed(tasks):
        results.append(await fut)
    print(results)


asyncio.run(main())
