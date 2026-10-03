"""Idle waits end at the earliest deadline, never a later one, on every target.

Each case keeps a far timer armed, so a wait capped at that timer, or a lost
deadline wake, shows up as a stall and prints False instead of True.
"""

import asyncio
import time

FAR = 30.0
SLACK = 5.0


def prompt(started):
    return time.monotonic() - started < SLACK


async def nearest_deadline_wins():
    loop = asyncio.get_running_loop()
    far = loop.call_later(FAR, print, "far timer must not fire")
    started = time.monotonic()
    await asyncio.sleep(0.2)
    elapsed = time.monotonic() - started
    far.cancel()
    print("sleep", elapsed >= 0.15, elapsed < SLACK)


async def deadline_armed_while_idle():
    loop = asyncio.get_running_loop()
    done = loop.create_future()
    far = loop.call_later(FAR, done.set_result, "far")

    def arm():
        # Runs from an earlier timer; the nearer deadline bounds the next wait.
        loop.call_later(0.05, done.set_result, "near")

    loop.call_later(0.05, arm)
    started = time.monotonic()
    result = await done
    far.cancel()
    print("rearm", result, prompt(started))


async def wait_for_timeout_cancels_a_far_sleep():
    started = time.monotonic()
    try:
        await asyncio.wait_for(asyncio.sleep(FAR), 0.1)
    except TimeoutError:
        print("timeout", prompt(started))


async def cancelled_timer_leaves_the_next_deadline_in_charge():
    loop = asyncio.get_running_loop()
    fired = []
    early = loop.call_later(0.05, fired.append, "early")
    loop.call_later(0.1, fired.append, "late")
    early.cancel()
    started = time.monotonic()
    await asyncio.sleep(0.2)
    print("cancelled", fired, prompt(started))


def run_forever_stops_at_timer():
    loop = asyncio.new_event_loop()
    try:
        far = loop.call_later(FAR, loop.stop)
        loop.call_later(0.1, loop.stop)
        started = time.monotonic()
        loop.run_forever()
        far.cancel()
        print("stop", prompt(started), loop.is_running())
    finally:
        loop.close()


asyncio.run(nearest_deadline_wins())
asyncio.run(deadline_armed_while_idle())
asyncio.run(wait_for_timeout_cancels_a_far_sleep())
asyncio.run(cancelled_timer_leaves_the_next_deadline_in_charge())
run_forever_stops_at_timer()
