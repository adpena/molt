"""Purpose: differential coverage for threading locks — Lock, RLock, acquire, release."""

import threading


# 1. Basic Lock acquire and release
lock = threading.Lock()
print("acquired", lock.acquire())
lock.release()
print("released-ok")


# 2. Lock acquire with timeout=0 (non-blocking)
lock = threading.Lock()
print("first-acquire", lock.acquire())
print("second-acquire-nonblock", lock.acquire(blocking=False))
lock.release()
print("after-release", lock.acquire(blocking=False))
lock.release()


# 3. Lock as context manager
lock = threading.Lock()
with lock:
    print("in-context-manager")
print("context-manager-released")


# 4. RLock can be acquired multiple times by same thread
rlock = threading.RLock()
print("rlock-1", rlock.acquire())
print("rlock-2", rlock.acquire())
rlock.release()
rlock.release()
print("rlock-released-ok")


# 5. RLock as context manager nested
rlock = threading.RLock()
with rlock:
    with rlock:
        print("nested-rlock-ok")
print("nested-rlock-released")


# 6. Release without acquire raises RuntimeError
lock = threading.Lock()
try:
    lock.release()
except RuntimeError as e:
    print(f"RuntimeError: {e}")


# 7. RLock release without acquire raises RuntimeError
rlock = threading.RLock()
try:
    rlock.release()
except RuntimeError as e:
    print(f"RuntimeError: {e}")


# 8. Lock contention — two threads, deterministic ordering
lock = threading.Lock()
order = []

def worker(name, lock, order):
    with lock:
        order.append(name)

lock.acquire()
t1 = threading.Thread(target=worker, args=("first", lock, order))
t2 = threading.Thread(target=worker, args=("second", lock, order))
t1.start()
t2.start()

import time
time.sleep(0.05)
lock.release()

t1.join()
t2.join()
print("both-finished", len(order) == 2)


# 9. Event set/wait
event = threading.Event()
print("event-is-set", event.is_set())
event.set()
print("event-is-set", event.is_set())
event.clear()
print("event-is-set", event.is_set())


# 10. Event wait with timeout
event = threading.Event()
result = event.wait(timeout=0.01)
print("wait-timeout", result)
event.set()
result = event.wait(timeout=0.01)
print("wait-set", result)


# 11. Barrier
barrier = threading.Barrier(3)
results = [None] * 3

def barrier_worker(idx, barrier, results):
    barrier.wait()
    results[idx] = f"passed-{idx}"

threads = []
for i in range(3):
    t = threading.Thread(target=barrier_worker, args=(i, barrier, results))
    threads.append(t)
    t.start()

for t in threads:
    t.join()

for r in sorted(results):
    print(r)
