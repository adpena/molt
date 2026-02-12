"""Purpose: verify timer cancel path stays intrinsic-backed and deterministic."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    fired: list[str] = []
    handle = loop.call_later(0.02, lambda: fired.append("fired"))
    handle.cancel()
    await asyncio.sleep(0.05)
    print("cancelled", handle.cancelled(), "fired", len(fired))


asyncio.run(main())
