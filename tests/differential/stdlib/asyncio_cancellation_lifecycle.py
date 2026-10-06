"""Representative cancellation/cleanup outcome. Stage for differential suite."""
import asyncio


async def chain(suppress):
    started = asyncio.Event()
    blocker = asyncio.Event()
    log = []

    async def child():
        started.set()
        try:
            await blocker.wait()
        except asyncio.CancelledError as exc:
            log.append(("cancel", exc.args))
            await asyncio.sleep(0)
            log.append("clean")
            if suppress:
                return "recovered"
            raise

    inner = asyncio.create_task(child())

    async def parent():
        return await inner

    outer = asyncio.create_task(parent())
    await started.wait()
    await asyncio.sleep(0)
    outer.cancel("chain")
    try:
        result = await outer
    except asyncio.CancelledError:
        result = "cancelled"
    print("chain", suppress, result, inner.cancelled(), outer.cancelled(), log)
    print("counts", inner.cancelling(), outer.cancelling(), inner.uncancel(), outer.uncancel())


async def independent_child():
    gate = asyncio.Event()
    started = asyncio.Event()
    children = []

    async def child():
        started.set()
        await gate.wait()
        return "survived"

    async def parent():
        children.append(asyncio.create_task(child()))
        await asyncio.Event().wait()

    parent_task = asyncio.create_task(parent())
    await started.wait()
    parent_task.cancel()
    try:
        await parent_task
    except asyncio.CancelledError:
        pass
    child_task = children[0]
    print("independent", child_task.done(), child_task.cancelling())
    gate.set()
    print("independent-result", await child_task)


async def shielded_child():
    gate = asyncio.Event()
    inner = asyncio.create_task(gate.wait())
    outer = asyncio.shield(inner)
    outer.cancel("shield")
    try:
        await outer
    except asyncio.CancelledError as exc:
        print("shield", exc.args, inner.done(), inner.cancelling())
    gate.set()
    print("shield-result", await inner)


async def gather_outcomes():
    gate = asyncio.Event()

    async def sibling():
        await gate.wait()
        return "sibling"

    async def fail():
        raise ValueError("expected")

    slow = asyncio.create_task(sibling())
    group = asyncio.gather(slow, fail())
    try:
        await group
    except ValueError:
        print("gather-error", slow.done(), slow.cancelling(), group.cancel())
    gate.set()
    print("gather-sibling", await slow)
    cleaned = []
    started = asyncio.Queue()

    async def cleanup(label):
        await started.put(label)
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned.append(label)

    one = asyncio.create_task(cleanup(1))
    two = asyncio.create_task(cleanup(2))
    group = asyncio.gather(one, two, return_exceptions=True)
    await started.get()
    await started.get()
    group.cancel("gather")
    try:
        await group
    except asyncio.CancelledError as exc:
        print("gather-cancel", exc.args, group.cancelled(), sorted(cleaned), one.cancelled(), two.cancelled())


async def multi_waiter():
    fut = asyncio.get_running_loop().create_future()

    async def wait():
        try:
            await fut
        except asyncio.CancelledError as exc:
            return exc.args

    first = asyncio.create_task(wait())
    second = asyncio.create_task(wait())
    await asyncio.sleep(0)
    fut.cancel(42)
    print("future-multi", await asyncio.gather(first, second))
    exceptional = asyncio.get_running_loop().create_future()
    exceptional.set_exception(asyncio.CancelledError("stored"))
    try:
        await exceptional
    except asyncio.CancelledError as exc:
        print("future-exception", exceptional.cancelled(), exc.args)


async def timeout_and_group():
    cleaned = []
    started = asyncio.Event()

    async def worker():
        started.set()
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned.append("timeout")

    task = asyncio.create_task(worker())
    await started.wait()
    try:
        async with asyncio.timeout(0):
            await task
    except TimeoutError:
        print("timeout", task.cancelled(), cleaned, asyncio.current_task().cancelling())

    async def fail():
        await asyncio.sleep(0)
        raise ValueError("group")

    async def sibling():
        try:
            await asyncio.Event().wait()
        finally:
            await asyncio.sleep(0)
            cleaned.append("group")

    try:
        async with asyncio.TaskGroup() as group:
            group.create_task(sibling())
            group.create_task(fail())
            await asyncio.Event().wait()
    except ExceptionGroup as exc:
        print("taskgroup", [type(err).__name__ for err in exc.exceptions], cleaned, asyncio.current_task().cancelling())


async def main():
    await chain(False)
    await chain(True)
    await independent_child()
    await shielded_child()
    await gather_outcomes()
    await multi_waiter()
    await timeout_and_group()


asyncio.run(main())
