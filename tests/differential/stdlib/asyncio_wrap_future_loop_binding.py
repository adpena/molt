"""Purpose: differential coverage for asyncio.wrap_future loop binding."""

import asyncio
from concurrent.futures import Future as ConcurrentFuture


loop = asyncio.new_event_loop()
try:
    source = ConcurrentFuture()
    wrapped = asyncio.wrap_future(source, loop=loop)
    print(wrapped.get_loop() is loop)
finally:
    loop.close()
