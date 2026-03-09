"""Purpose: equal-deadline loop.call_at callbacks preserve FIFO order."""

import asyncio
import sys


async def main() -> None:
    if sys.platform == "win32":
        print("unsupported")
        return
    loop = asyncio.get_running_loop()
    observed: list[str] = []
    when = loop.time() + 0.02

    loop.call_at(when, observed.append, "first")
    loop.call_at(when, observed.append, "second")
    loop.call_at(when, observed.append, "third")

    await asyncio.sleep(0.05)
    print(observed)
    assert observed == ["first", "second", "third"], observed


asyncio.run(main())
