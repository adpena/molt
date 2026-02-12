"""Purpose: differential coverage for queue timeout basic."""

import queue


q: queue.Queue[int] = queue.Queue()

try:
    q.get(timeout=0.05)
except Exception as exc:
    print(type(exc).__name__)

q.put(1)
print(q.get_nowait())
