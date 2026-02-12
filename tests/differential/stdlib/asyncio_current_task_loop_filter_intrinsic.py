import asyncio


async def probe():
    loop = asyncio.get_running_loop()
    current = asyncio.current_task(loop)
    other_loop = asyncio.new_event_loop()
    try:
        mismatch = asyncio.current_task(other_loop)
    finally:
        other_loop.close()
    print("ASYNCIO", isinstance(current, asyncio.Task), mismatch is None)


asyncio.run(probe())
