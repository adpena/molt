"""Purpose: differential coverage for asyncio Runner lifecycle."""

import asyncio


async def main() -> str:
    await asyncio.sleep(0)
    return "ok"


with asyncio.Runner() as runner:
    result = runner.run(main())

print(result)
