"""Purpose: stop() scheduled via call_soon terminates run_forever (no spin/OOM)."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    events: list[object] = []

    def stopper() -> None:
        events.append("callback")
        loop.stop()

    loop.call_soon(stopper)
    loop.run_forever()
    events.append(("running", loop.is_running()))
    print(events)
finally:
    asyncio.set_event_loop(None)
    loop.close()
