"""Purpose: differential coverage for Task.cancelling/uncancel behavior."""

import asyncio


async def worker(log: list[tuple[str, int | str | bool]]) -> str:
    task = asyncio.current_task()
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        log.append(("cancelling", task.cancelling()))
        while task.cancelling():
            remaining = task.uncancel()
            log.append(("uncancel", remaining))
        return "recovered"
    return "ok"


async def main() -> None:
    log: list[tuple[str, int | str | bool]] = []
    task = asyncio.create_task(worker(log))
    await asyncio.sleep(0)
    task.cancel("first")
    task.cancel("second")
    result = await task
    log.append(("result", result, task.cancelled(), task.cancelling()))
    print(log)


asyncio.run(main())
