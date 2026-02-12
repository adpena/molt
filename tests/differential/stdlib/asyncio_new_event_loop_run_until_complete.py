"""Purpose: differential coverage for new_event_loop + run_until_complete."""

import asyncio


async def main() -> str:
    await asyncio.sleep(0)
    return "ok"


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    result = loop.run_until_complete(main())
    print(result, loop.is_running())
finally:
    loop.close()
    asyncio.set_event_loop(None)

print(loop.is_closed())
