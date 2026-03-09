"""Purpose: pre-stopped run_forever executes one ready turn and returns."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    events: list[object] = []
    loop.call_soon(events.append, "ready")
    loop.stop()
    loop.run_forever()
    events.append(("running", loop.is_running()))
    print(events)
finally:
    loop.close()
    asyncio.set_event_loop(None)
