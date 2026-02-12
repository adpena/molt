"""Purpose: differential coverage for asyncio shield + wait_for interplay."""

import asyncio


async def worker(log: list[str]) -> str:
    await asyncio.sleep(0)
    log.append("done")
    return "ok"


async def main() -> None:
    log: list[object] = []
    task = asyncio.create_task(worker(log))
    await asyncio.sleep(0)
    try:
        await asyncio.wait_for(asyncio.shield(task), timeout=0)
    except asyncio.TimeoutError:
        log.append("timeout")
    await asyncio.sleep(0)
    log.append(("cancelled", task.cancelled(), "done", task.done()))
    if task.done() and not task.cancelled():
        log.append(("result", task.result()))
    print(log)


asyncio.run(main())
