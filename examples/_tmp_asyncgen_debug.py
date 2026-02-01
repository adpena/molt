import asyncio
import inspect


async def agen():
    token = "t"
    await asyncio.sleep(0)
    yield token


async def main():
    it = agen()
    print("initial_ag_await", it.ag_await)
    task = asyncio.create_task(it.__anext__())
    await asyncio.sleep(0)
    print("after_sleep_ag_running", it.ag_running)
    print("after_sleep_ag_await", it.ag_await)
    if it.ag_await:
        print("ag_await_type", type(it.ag_await))
        print("ag_await_type_name", getattr(type(it.ag_await), "__name__", "<missing>"))
        print(
            "ag_await_class_name",
            getattr(it.ag_await.__class__, "__name__", "<missing>"),
        )
        print("ag_await_locals", inspect.getasyncgenlocals(it))
    try:
        val = await task
        print("val", val)
    except Exception as exc:
        print("task_exc", type(exc).__name__, exc)


asyncio.run(main())
