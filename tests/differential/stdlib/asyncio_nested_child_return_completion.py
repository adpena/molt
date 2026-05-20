"""Purpose: differential coverage for nested asyncio child return completion."""

import asyncio


async def child() -> str:
    print("child")
    return "x"


async def main() -> None:
    print("before")
    value = await child()
    print("after", value)


asyncio.run(main())
print("done")
