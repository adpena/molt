"""Purpose: equal-delay loop.call_later callbacks preserve FIFO order."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    observed: list[str] = []

    loop.call_later(0.02, observed.append, "first")
    loop.call_later(0.02, observed.append, "second")
    loop.call_later(0.02, observed.append, "third")

    await asyncio.sleep(0.05)
    print(observed)
    assert observed == ["first", "second", "third"], observed


asyncio.run(main())
