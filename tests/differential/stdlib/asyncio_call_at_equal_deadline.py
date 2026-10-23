"""Purpose: equal-deadline loop.call_at callbacks all run by their deadline.

CPython documents the order of equal-deadline callbacks as undefined: its
heapq timer heap orders by deadline alone, and a later timer changes how the
heap sifts. The program compares only which callbacks ran. Molt's FIFO order
is pinned by the runtime unit test equal_deadline_timers_run_in_registration_order.
"""

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
    print(sorted(observed))


asyncio.run(main())
