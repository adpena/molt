import asyncio

async def main():
    x = 40
    await asyncio.sleep(0)
    print(x + 2)

asyncio.run(main())
