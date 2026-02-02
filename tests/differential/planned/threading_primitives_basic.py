"""Purpose: differential coverage for threading primitives."""

import threading

lock = threading.Lock()
print(lock.acquire())
print(lock.locked())
lock.release()

rlock = threading.RLock()
print(rlock.acquire())
print(rlock.acquire())
rlock.release()
rlock.release()

ready = threading.Event()
print(ready.is_set())
ready.set()
print(ready.is_set())
ready.clear()
print(ready.is_set())

results: list[str] = []

def worker():
    results.append("start")
    ready.wait(1)
    results.append("done")

thread = threading.Thread(target=worker)
thread.start()
ready.set()
thread.join()

print(results)
