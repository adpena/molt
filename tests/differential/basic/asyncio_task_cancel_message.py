"""Purpose: differential coverage for task cancel message propagation."""

import asyncio


async def child(log: list[tuple[str, tuple[object, ...]]]) -> None:
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError as exc:
        log.append(("child", exc.args))
        raise


async def main() -> None:
    log: list[tuple[str, tuple[object, ...]]] = []
    task = asyncio.create_task(child(log))
    await asyncio.sleep(0)
    task.cancel("bye")
    try:
        await task
    except asyncio.CancelledError as exc:
        log.append(("main", exc.args))
    print(log, task.cancelled())


asyncio.run(main())
