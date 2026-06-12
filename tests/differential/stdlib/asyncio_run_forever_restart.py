"""Purpose: run_forever can be re-entered after stop() (loop drives twice)."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    order: list[str] = []

    def first() -> None:
        order.append("first")
        loop.stop()

    def second() -> None:
        order.append("second")
        loop.stop()

    loop.call_soon(first)
    loop.run_forever()
    order.append("between")
    loop.call_soon(second)
    loop.run_forever()
    print(order, loop.is_running())
finally:
    asyncio.set_event_loop(None)
    loop.close()
