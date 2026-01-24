"""Purpose: differential coverage for asyncio event loop policy basic."""

import asyncio


async def main() -> None:
    policy = asyncio.get_event_loop_policy()
    asyncio.set_event_loop_policy(policy)
    print(type(policy).__name__)


asyncio.run(main())
