"""Purpose: differential coverage for asyncio task registry errors outside a loop."""

import asyncio


def _probe(name: str, fn) -> None:
    try:
        fn()
    except Exception as exc:
        print(name, type(exc).__name__, str(exc))
    else:
        print(name, "ok")


_probe("current_task", asyncio.current_task)
_probe("all_tasks", asyncio.all_tasks)
