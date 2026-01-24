"""Purpose: differential coverage for async with basic."""

import asyncio


class AsyncCM:
    def __init__(self) -> None:
        self.events: list[str] = []

    async def __aenter__(self) -> "AsyncCM":
        self.events.append("enter")
        return self

    async def __aexit__(self, exc_type, exc, tb) -> bool:
        self.events.append("exit")
        return False


async def main() -> str:
    ctx = AsyncCM()
    async with ctx as cm:
        cm.events.append("body")
    return ",".join(ctx.events)


print(asyncio.run(main()))
