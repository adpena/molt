import asyncio


async def main():
    x = asyncio.sleep(0)
    print("sleep_obj", x)
    print("type_sleep", type(x))
    print("type_sleep_name", type(x).__name__)
    await asyncio.sleep(0)


asyncio.run(main())
