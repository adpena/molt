"""Overdue sleeps, timeout callbacks and follow-up callbacks share timer order."""
import asyncio
import time


async def nested(depth, wait):
    if depth:
        return await nested(depth - 1, wait)
    return await wait


async def main():
    loop = asyncio.get_running_loop()
    events = []

    def earlier_timer():
        events.append("timer")
        loop.call_soon(events.append, "timer-followup")

    # Both timers become due while one callback occupies the loop. Task wakeup
    # belongs after the earlier callback's newly scheduled follow-up.
    loop.call_later(0.002, earlier_timer)
    loop.call_soon(time.sleep, 0.020)
    await asyncio.sleep(0.004)
    events.append("sleep")
    print("overdue-order", events)

    try:
        async with asyncio.timeout(0.002):
            loop.call_soon(time.sleep, 0.020)
            await nested(64, asyncio.sleep(0.004))
    except TimeoutError:
        print("timeout-first", True)
    else:
        print("timeout-first", False)

    future = loop.create_future()
    loop.call_later(0.002, future.set_result, 42)
    print("deep-promise", await nested(64, future))


asyncio.run(main())
