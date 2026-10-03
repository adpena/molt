# MOLT_ENV: MOLT_CAPABILITIES=thread
# MOLT_META: backends=llvm,native
"""Cross-thread publications end an idle loop's wait at once.

Every case parks behind a far timer, so a lost wake stalls until that timer
and prints False instead of True.
"""

import asyncio
import threading
import time

FAR = 30.0
PROMPT = 5.0


def prompt(started):
    return time.monotonic() - started < PROMPT


def start_worker(work):
    thread = threading.Thread(target=work)
    thread.start()
    return thread


async def threadsafe_callback(loop):
    fut = loop.create_future()

    def work():
        time.sleep(0.05)
        loop.call_soon_threadsafe(fut.set_result, "threadsafe")

    started = time.monotonic()
    thread = start_worker(work)
    value = await fut
    thread.join()
    return value, prompt(started)


async def executor_completion(loop):
    started = time.monotonic()
    value = await loop.run_in_executor(None, time.sleep, 0.05)
    return value, prompt(started)


async def threadsafe_timer(loop):
    fired = loop.create_future()

    def work():
        time.sleep(0.05)
        loop.call_soon_threadsafe(loop.call_later, 0.05, fired.set_result, "timer")

    started = time.monotonic()
    thread = start_worker(work)
    value = await fired
    thread.join()
    return value, prompt(started)


async def threadsafe_cancel(loop):
    entered = asyncio.Event()

    async def sleeper():
        entered.set()
        await asyncio.sleep(FAR)

    task = loop.create_task(sleeper())
    await entered.wait()

    def work():
        time.sleep(0.05)
        loop.call_soon_threadsafe(task.cancel, "from-thread")

    started = time.monotonic()
    thread = start_worker(work)
    try:
        await task
    except asyncio.CancelledError as exc:
        outcome = exc.args
    thread.join()
    return outcome, prompt(started)


async def threadsafe_fifo(loop):
    order = []
    done = loop.create_future()

    def work():
        time.sleep(0.05)
        for index in range(5):
            loop.call_soon_threadsafe(order.append, index)
        loop.call_soon_threadsafe(done.set_result, None)

    thread = start_worker(work)
    await done
    thread.join()
    return order


async def burst_of_publications_while_parked(loop):
    done = loop.create_future()
    seen = []

    def work():
        time.sleep(0.05)
        for index in range(200):
            loop.call_soon_threadsafe(seen.append, index)
        loop.call_soon_threadsafe(done.set_result, None)

    started = time.monotonic()
    thread = start_worker(work)
    await done
    thread.join()
    return len(seen), seen == list(range(200)), prompt(started)


async def main():
    loop = asyncio.get_running_loop()
    far = loop.call_later(FAR, print, "far timer must not fire")
    try:
        print("threadsafe", await threadsafe_callback(loop))
        print("executor", await executor_completion(loop))
        print("timer", await threadsafe_timer(loop))
        print("cancel", await threadsafe_cancel(loop))
        print("fifo", await threadsafe_fifo(loop))
        print("burst", await burst_of_publications_while_parked(loop))
    finally:
        far.cancel()


def stop_from_thread():
    loop = asyncio.new_event_loop()
    try:
        far = loop.call_later(FAR, print, "far timer must not fire")

        def work():
            time.sleep(0.05)
            loop.call_soon_threadsafe(loop.stop)

        started = time.monotonic()
        thread = start_worker(work)
        loop.run_forever()
        thread.join()
        far.cancel()
        print("stop", prompt(started), loop.is_running())
    finally:
        loop.close()


def restart_after_stop_parks_again():
    loop = asyncio.new_event_loop()
    try:
        loop.stop()
        loop.run_forever()
        done = loop.create_future()

        def work():
            time.sleep(0.05)
            loop.call_soon_threadsafe(done.set_result, "second run")

        started = time.monotonic()
        thread = start_worker(work)
        value = loop.run_until_complete(done)
        thread.join()
        print("restart", value, prompt(started))
    finally:
        loop.close()


asyncio.run(main())
stop_from_thread()
restart_after_stop_parks_again()
