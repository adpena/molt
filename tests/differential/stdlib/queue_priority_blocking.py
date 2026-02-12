"""Purpose: differential coverage for blocking PriorityQueue ordering/progress."""

import queue
import threading


q: queue.PriorityQueue[int] = queue.PriorityQueue(maxsize=1)
out: list[int] = []


def producer() -> None:
    q.put(2)
    q.put(1)


def consumer() -> None:
    out.append(q.get())
    q.task_done()
    out.append(q.get())
    q.task_done()


t1 = threading.Thread(target=producer)
t2 = threading.Thread(target=consumer)
t1.start()
t2.start()
q.join()
t1.join()
t2.join()
print(out)
