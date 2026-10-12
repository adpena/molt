# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,thread.spawn,thread.shared
"""Both public APIs preserve distinct callable arguments and retained targets."""
import _thread
import gc
import threading

release = threading.Event()
finished = threading.Event()
lock = threading.Lock()
results = []


def record(label, *, value):
    release.wait()
    with lock:
        results.append((label, value))
        if len(results) == 3:
            finished.set()


first = threading.Thread(target=record, args=("threading-first",), kwargs={"value": 11})
first.start()
_thread.start_new_thread(record, ("low-level",), {"value": 22})
last = threading.Thread(target=record, args=("threading-last",), kwargs={"value": 33})
last.start()
del first
gc.collect()
release.set()
assert finished.wait(10), "worker lost its admitted call"
last.join()
print(sorted(results))
