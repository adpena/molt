import asyncio


async def main() -> int:
    total = 0
    for i in range(3):
        await asyncio.sleep(0)
        total += i
    return total


print(asyncio.run(main()))
