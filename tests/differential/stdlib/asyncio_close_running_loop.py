"""Purpose: running event loops must reject close()."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)

    def attempt_close() -> None:
        try:
            loop.close()
        except Exception as exc:
            print(type(exc).__name__, str(exc))
        finally:
            loop.stop()

    loop.call_soon(attempt_close)
    loop.run_forever()
finally:
    asyncio.set_event_loop(None)
    if not loop.is_closed():
        loop.close()
