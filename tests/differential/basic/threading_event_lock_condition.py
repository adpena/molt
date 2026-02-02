"""Purpose: differential coverage for threading event lock condition."""

import threading


order: list[str] = []
ready = threading.Event()
lock = threading.Lock()
cond = threading.Condition()


def worker() -> None:
    with lock:
        order.append("locked")
    with cond:
        ready.set()
        ok = cond.wait_for(lambda: ready.is_set(), timeout=0.5)
        order.append(f"wait_for:{ok}")


t = threading.Thread(target=worker)

t.start()
ready.wait(timeout=0.5)
with cond:
    cond.notify_all()

t.join()
print(order)
print("event_set", ready.is_set())
