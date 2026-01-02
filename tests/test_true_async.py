import asyncio

async def main():
    print(1)
    await asyncio.sleep(0)
    print(2)

asyncio.run(main())