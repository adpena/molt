"""Purpose: differential coverage for asyncio.ensure_future loop mismatch."""

import asyncio


loop1 = asyncio.new_event_loop()
loop2 = asyncio.new_event_loop()
try:
    fut = loop1.create_future()
    try:
        asyncio.ensure_future(fut, loop=loop2)
    except Exception as exc:
        print(type(exc).__name__, str(exc))
    else:
        print("accepted")
finally:
    loop1.close()
    loop2.close()
