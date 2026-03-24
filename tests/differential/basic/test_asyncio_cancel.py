"""Purpose: differential coverage for asyncio task cancellation and CancelledError."""

import asyncio


# 1. Basic task cancellation
async def test_basic_cancel():
    async def long_sleep():
        await asyncio.sleep(10)

    task = asyncio.create_task(long_sleep())
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        print("basic-cancel-caught")
    print("task-cancelled", task.cancelled())

asyncio.run(test_basic_cancel())


# 2. Cancel with message
async def test_cancel_msg():
    async def worker():
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError as e:
            print("cancel-msg", repr(e))
            raise

    task = asyncio.create_task(worker())
    await asyncio.sleep(0)
    task.cancel("custom-reason")
    try:
        await task
    except asyncio.CancelledError:
        pass

asyncio.run(test_cancel_msg())


# 3. Shielded task
async def test_shield():
    async def shielded_work():
        await asyncio.sleep(0)
        return "shielded-result"

    task = asyncio.create_task(shielded_work())
    shielded = asyncio.shield(task)
    await asyncio.sleep(0)
    shielded.cancel()
    try:
        await shielded
    except asyncio.CancelledError:
        print("shield-outer-cancelled")
    result = await task
    print("shield-inner", result)

asyncio.run(test_shield())


# 4. Cancel during gather
async def test_cancel_gather():
    async def slow(n):
        await asyncio.sleep(10)
        return n

    tasks = [asyncio.create_task(slow(i)) for i in range(3)]
    gather = asyncio.gather(*tasks)
    await asyncio.sleep(0)
    for t in tasks:
        t.cancel()
    try:
        await gather
    except asyncio.CancelledError:
        print("gather-cancelled")

asyncio.run(test_cancel_gather())


# 5. CancelledError is a BaseException subclass
print("cancelled-is-base", issubclass(asyncio.CancelledError, BaseException))


# 6. Task exception after cancel
async def test_cancel_exception():
    async def worker():
        await asyncio.sleep(10)

    task = asyncio.create_task(worker())
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    print("task-cancelled-check", task.cancelled())

asyncio.run(test_cancel_exception())


# 7. Cancel already finished task
async def test_cancel_finished():
    async def quick():
        return "quick-done"

    task = asyncio.create_task(quick())
    await task
    result = task.cancel()
    print("cancel-finished", result)
    print("still-done", task.done())
    print("not-cancelled", task.cancelled())

asyncio.run(test_cancel_finished())


# 8. Nested cancellation — inner catches and suppresses
async def test_nested_cancel():
    async def inner():
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            print("inner-suppressed")
            return "recovered"

    task = asyncio.create_task(inner())
    await asyncio.sleep(0)
    task.cancel()
    result = await task
    print("outer-got", result)

asyncio.run(test_nested_cancel())
