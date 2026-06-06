"""Purpose: differential coverage for asyncio basics — async/await, gather, sleep, create_task."""

import asyncio


# 1. Simple async function
async def hello():
    return "hello-async"

print(asyncio.run(hello()))


# 2. Await another coroutine
async def inner():
    return 42

async def outer():
    val = await inner()
    print("inner-result", val)

asyncio.run(outer())


# 3. asyncio.sleep
async def sleeper():
    await asyncio.sleep(0)
    print("sleep-done")

asyncio.run(sleeper())


# 4. asyncio.gather — results in order
async def make_value(n):
    await asyncio.sleep(0)
    return n * 10

async def gather_test():
    results = await asyncio.gather(make_value(1), make_value(2), make_value(3))
    print("gather", results)

asyncio.run(gather_test())


# 5. create_task
async def task_worker(name):
    await asyncio.sleep(0)
    return f"task-{name}"

async def task_test():
    t1 = asyncio.create_task(task_worker("a"))
    t2 = asyncio.create_task(task_worker("b"))
    r1 = await t1
    r2 = await t2
    print("tasks", r1, r2)

asyncio.run(task_test())


# 6. Task done/result
async def done_test():
    async def worker():
        return "finished"

    task = asyncio.create_task(worker())
    await task
    print("task-done", task.done())
    print("task-result", task.result())

asyncio.run(done_test())


# 7. Exception in coroutine
async def failing():
    raise ValueError("async-boom")

async def catch_test():
    try:
        await failing()
    except ValueError as e:
        print(f"caught: {e}")

asyncio.run(catch_test())


# 8. Gather with return_exceptions
async def good():
    return "ok"

async def bad():
    raise RuntimeError("fail")

async def gather_exc():
    results = await asyncio.gather(good(), bad(), return_exceptions=True)
    for r in results:
        if isinstance(r, Exception):
            print(f"exc: {type(r).__name__}: {r}")
        else:
            print(f"val: {r}")

asyncio.run(gather_exc())


# 9. Nested coroutine calls
async def level3():
    return "deep"

async def level2():
    return await level3()

async def level1():
    result = await level2()
    print("nested", result)

asyncio.run(level1())


# 10. Coroutine with multiple awaits
async def multi_await():
    a = await asyncio.sleep(0)
    b = await inner()
    print("multi-await", a, b)

asyncio.run(multi_await())
