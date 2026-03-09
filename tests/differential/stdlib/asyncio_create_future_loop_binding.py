"""Purpose: differential coverage for asyncio loop.create_future loop binding."""

import asyncio


loop = asyncio.new_event_loop()
try:
    fut = loop.create_future()
    print(fut.get_loop() is loop)
finally:
    loop.close()
