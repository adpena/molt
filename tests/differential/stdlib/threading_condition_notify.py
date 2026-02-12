"""Purpose: differential coverage for threading condition notify."""

import threading


cond = threading.Condition()
order: list[str] = []


def waiter() -> None:
    with cond:
        cond.wait(timeout=0.5)
        order.append("woke")


t = threading.Thread(target=waiter)
t.start()
with cond:
    cond.notify()

t.join()
print(order)
