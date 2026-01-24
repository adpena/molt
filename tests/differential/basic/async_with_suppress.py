"""Purpose: differential coverage for async with suppress."""

import asyncio


class SuppressCM:
    async def __aenter__(self) -> int:
        return 7

    async def __aexit__(self, exc_type, exc, tb) -> bool:
        return True


async def main() -> int:
    async with SuppressCM() as val:
        raise ValueError("boom")
    return val


print(asyncio.run(main()))
