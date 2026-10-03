"""Canceled timers never fire and promptly release captured callback objects."""

import asyncio
import weakref


async def main() -> None:
    loop = asyncio.get_running_loop()
    fired: list[str] = []
    handle = loop.call_later(0.02, lambda: fired.append("fired"))
    handle.cancel()
    await asyncio.sleep(0.05)
    print("cancelled", handle.cancelled(), "fired", len(fired))

    class Payload:
        def run(self):
            fired.append("unexpected")

    objects = []
    for _ in range(1_000):
        payload = Payload()
        objects.append(weakref.ref(payload))
        timer = loop.call_later(86_400, payload.run)
        timer.cancel()
        timer.cancel()
        del payload, timer
    await asyncio.sleep(0)
    print("released-callbacks", sum(ref() is None for ref in objects))
    print("still-no-callbacks", len(fired))


asyncio.run(main())
