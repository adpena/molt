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
    await projection_cases()


class WrappedAwait:
    def __init__(self, coroutine):
        self.coroutine = coroutine
        self.iterator = None

    def __await__(self):
        self.iterator = self.coroutine.__await__()
        return self.iterator


async def projection_body(delegate, __await_future_user):
    await delegate
    yield __await_future_user


async def nested_wait(started, release):
    started.set_result(None)
    await release
    await asyncio.sleep(0)


async def projection_cases():
    for wrapped in (False, True):
        loop = asyncio.get_running_loop()
        started = loop.create_future()
        release = loop.create_future()
        coroutine = nested_wait(started, release)
        delegate = WrappedAwait(coroutine) if wrapped else coroutine
        marker = object()
        generator = projection_body(delegate, marker)
        task = asyncio.create_task(generator.__anext__())
        await started
        acquired = delegate.iterator if wrapped else coroutine
        print("projection-suspended", wrapped, generator.ag_await is acquired,
              generator.ag_await is not marker, generator.ag_running)
        release.set_result(None)
        # Waking a Future does not resume the awaiting continuation synchronously.
        print("projection-woken", wrapped, generator.ag_await is acquired)
        result = await task
        print("projection-yielded", wrapped, result is marker,
              generator.ag_await is None, not generator.ag_running,
              inspect.getcoroutinestate(coroutine) == "CORO_CLOSED")
        await generator.aclose()
        print("projection-closed", wrapped, generator.ag_await is None,
              generator.ag_frame is None)


asyncio.run(main())
