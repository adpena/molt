"""Purpose: differential coverage for threading negative timeout semantics."""

import threading
import time


gate = threading.Event()


def sleeper() -> None:
    gate.wait()


t = threading.Thread(target=sleeper)
t.start()
t.join(-1)
print("join_neg_alive", t.is_alive())
t.join(-0.1)
print("join_neg_small_alive", t.is_alive())
gate.set()
t.join(1.0)
print("join_done", not t.is_alive())

cond = threading.Condition()
with cond:
    print("cond_wait_neg", cond.wait(-1))

event = threading.Event()
print("event_wait_neg", event.wait(-1))

sem = threading.Semaphore(0)
print("sem_wait_neg", sem.acquire(timeout=-1))

try:
    sem.acquire(blocking=False, timeout=0)
except Exception as exc:
    print("sem_nonblock_timeout_exc", type(exc).__name__)

# keep one tiny delay to reduce timing flake across runtimes
time.sleep(0.001)
