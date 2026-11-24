# MOLT_ENV: MOLT_CAPABILITIES=thread.spawn,thread.shared
# MOLT_META: backends=llvm,native platforms=posix
"""A frame inspected on another thread retains its actual executing caller."""

import sys
import threading


def observe_worker(generator):
    ready = threading.Event()
    resume = threading.Event()
    frames = []
    tasks = []

    def wait_here():
        frames.append(sys._getframe())
        ready.set()
        assert resume.wait(10)

    def yielding():
        wait_here()
        yield 1

    def run():
        if generator:
            task = yielding()
            tasks.append(task)
            next(task)
            frames.append(task.gi_frame.f_back)
            task.close()
        else:
            wait_here()
        frames.append(sys._getframe())

    worker = threading.Thread(target=run)
    worker.start()
    try:
        assert ready.wait(10)
        # run and yielding have not observed themselves. Their first frame
        # objects are requested from a different thread's stack.
        parent = frames[0].f_back
        if generator:
            same_generator = parent is tasks[0].gi_frame
            parent = parent.f_back
    finally:
        resume.set()
        worker.join()
    if generator:
        return same_generator, parent is frames[-1], frames[1] is None
    return parent is frames[-1]


print(observe_worker(False), observe_worker(True))
