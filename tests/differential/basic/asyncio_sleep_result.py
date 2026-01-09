import asyncio


async def main() -> int:
    first = await asyncio.sleep(0, result=3)
    second = await asyncio.sleep(0.0, 4)
    return first + second


print(asyncio.run(main()))
