"""Purpose: differential coverage for async with instance callable."""

import asyncio


class AsyncCM:
    async def __aenter__(self) -> str:
        return "class"

    async def __aexit__(self, exc_type, exc, tb) -> bool:
        return False


async def main() -> str:
    ctx = AsyncCM()

    async def enter_override() -> str:
        return "instance"

    async def exit_override(exc_type, exc, tb) -> bool:
        return False

    ctx.__aenter__ = enter_override
    ctx.__aexit__ = exit_override

    async with ctx as val:
        return val


print(asyncio.run(main()))
