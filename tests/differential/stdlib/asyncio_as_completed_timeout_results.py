"""Purpose: differential coverage for asyncio.as_completed timeout/result semantics."""

import asyncio


async def worker(label: str, delay: float) -> str:
    await asyncio.sleep(delay)
    return label


async def main() -> None:
    tasks = [
        asyncio.create_task(worker("fast", 0.01)),
        asyncio.create_task(worker("slow", 0.08)),
    ]
    out: list[tuple[str, str]] = []
    for fut in asyncio.as_completed(tasks, timeout=0.04):
        try:
            value = await fut
            out.append((value, type(value).__name__))
        except TimeoutError as exc:
            out.append(("timeout", type(exc).__name__))
    await asyncio.gather(*tasks, return_exceptions=True)
    print(out)


asyncio.run(main())
