"""Purpose: set_task_factory rejects non-callables."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    for value in (123, object(), "x"):
        try:
            loop.set_task_factory(value)
        except Exception as exc:
            print(type(exc).__name__, str(exc))
finally:
    loop.close()
    asyncio.set_event_loop(None)
