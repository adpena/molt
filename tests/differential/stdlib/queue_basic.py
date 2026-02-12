"""Purpose: differential coverage for queue basic."""

import queue
import threading


q: queue.Queue[str] = queue.Queue(maxsize=1)
results: list[str] = []


def producer() -> None:
    q.put("a")
    q.put("b")


def consumer() -> None:
    results.append(q.get())
    q.task_done()
    results.append(q.get())
    q.task_done()


t1 = threading.Thread(target=producer)
t2 = threading.Thread(target=consumer)

t1.start()
t2.start()

q.join()

t1.join()
t2.join()

print(results, q.qsize())
