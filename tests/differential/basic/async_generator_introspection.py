"""Purpose: differential coverage for async generator introspection APIs."""

import asyncio
import inspect


async def agen():
    token = "t"
    await asyncio.sleep(0)
    yield token


async def main() -> None:
    it = agen()
    print("code", it.ag_code.co_name)
    print("frame0_none", it.ag_frame is None)
    print("running0", it.ag_running)
    print("await0_none", it.ag_await is None)
    print("locals0", sorted(inspect.getasyncgenlocals(it).keys()))
    task = asyncio.create_task(it.__anext__())
    await asyncio.sleep(0)
    print("running1", it.ag_running)
    print("await1_none", it.ag_await is None)
    print("await1_type", type(it.ag_await).__name__ if it.ag_await else None)
    print("frame1_none", it.ag_frame is None)
    print("locals1", sorted(inspect.getasyncgenlocals(it).keys()))
    val = await task
    print("val", val)
    print("running2", it.ag_running)
    print("await2_none", it.ag_await is None)
    print("frame2_none", it.ag_frame is None)
    print("state2", inspect.getasyncgenstate(it))
    try:
        await it.__anext__()
    except Exception as exc:
        print("done", type(exc).__name__)
    print("frame3_none", it.ag_frame is None)
    print("locals3", inspect.getasyncgenlocals(it))


asyncio.run(main())
