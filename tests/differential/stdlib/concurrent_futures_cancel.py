"""Purpose: differential coverage for concurrent futures cancel."""

from concurrent.futures import ThreadPoolExecutor
from threading import Event


with ThreadPoolExecutor(max_workers=1) as executor:
    started = Event()
    release = Event()

    def blocked():
        started.set()
        release.wait()

    running = executor.submit(blocked)
    started.wait()
    fut = executor.submit(lambda: 1)
    callbacks = []
    fut.add_done_callback(lambda done: callbacks.append(done.cancelled()))
    cancelled = fut.cancel()
    print(cancelled, fut.cancelled())
    print("repeat", fut.cancel(), callbacks)
    release.set()
    running.result()
