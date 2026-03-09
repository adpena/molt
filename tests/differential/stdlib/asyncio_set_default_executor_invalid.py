"""Purpose: differential coverage for asyncio.set_default_executor validation."""

import asyncio
from concurrent.futures import ThreadPoolExecutor


class DummyExecutor:
    def submit(self, fn, *args):
        return fn(*args)


loop = asyncio.new_event_loop()
try:
    for value in (None, object(), DummyExecutor()):
        try:
            loop.set_default_executor(value)
        except Exception as exc:
            print(type(exc).__name__, str(exc))
        else:
            print("accepted", type(value).__name__)
    with ThreadPoolExecutor(max_workers=1) as executor:
        loop.set_default_executor(executor)
        print("valid", isinstance(executor, ThreadPoolExecutor))
finally:
    loop.close()
