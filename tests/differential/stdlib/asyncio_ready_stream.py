"""One ordered ready stream across drivers, tasks, callbacks, and wakeups."""

import asyncio
import contextvars


loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
events = []
marker = contextvars.ContextVar("marker", default="outside")
errors = []


def first():
    events.append("first")
    loop.call_soon(events.append, "second")


async def complete():
    events.append("task-start")
    loop.call_soon(events.append, "callback-before-resume")
    await asyncio.sleep(0)
    events.append("task-resume")
    return 42


def report(loop_arg, context):
    errors.append((type(context["exception"]).__name__, marker.get()))


def fail():
    raise ValueError("callback failure")


try:
    loop.call_soon(first)
    loop.call_soon(loop.stop)
    loop.run_forever()
    print("first-turn", events)
    print("result", loop.run_until_complete(complete()))
    print("ordered", events)

    loop.set_exception_handler(report)
    marker.set("scheduled")
    loop.call_soon(fail)
    marker.set("outside")
    loop.call_soon(events.append, "survivor")
    loop.call_soon(loop.stop)
    loop.run_forever()
    print("error-context", errors, events[-1])

    future = loop.create_future()
    marker.set("future-context")
    future.add_done_callback(lambda done: events.append((done.result(), marker.get())))
    marker.set("outside")
    future.set_result("future-done")
    print("future-deferred", events[-1])
    loop.run_until_complete(future)
    print("future-delivered", events[-1])

    def fatal():
        raise KeyboardInterrupt()

    loop.call_soon(fatal)
    loop.call_soon(events.append, "after-fatal")
    loop.call_soon(loop.stop)
    try:
        loop.run_forever()
    except KeyboardInterrupt:
        print("fatal-propagated", True)
    loop.run_forever()
    print("remaining-batch", events[-1])

    for exception in (SystemExit, KeyboardInterrupt):
        async def fatal_task():
            raise exception("task fatal")

        task = loop.create_task(fatal_task())
        try:
            loop.run_until_complete(task)
        except BaseException as caught:
            print("fatal-task", type(caught).__name__, task.done(), task.exception() is caught)
        print("driver-restored", loop.is_running(), asyncio.current_task(loop))
        print("restart", loop.run_until_complete(asyncio.sleep(0, result=17)))
finally:
    loop.close()
    asyncio.set_event_loop(None)
