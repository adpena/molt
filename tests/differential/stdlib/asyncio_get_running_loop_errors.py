"""Purpose: differential coverage for get_running_loop errors."""

import asyncio


try:
    asyncio.get_running_loop()
except RuntimeError as exc:
    print(type(exc).__name__)
