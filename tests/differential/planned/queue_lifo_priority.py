"""Purpose: differential coverage for queue LIFO/PriorityQueue behavior."""

import queue


lifo: queue.LifoQueue[int] = queue.LifoQueue()
lifo.put(1)
lifo.put(2)
lifo.put(3)
print([lifo.get(), lifo.get(), lifo.get()])

prio: queue.PriorityQueue[int] = queue.PriorityQueue()
prio.put(5)
prio.put(1)
prio.put(3)
print([prio.get(), prio.get(), prio.get()])

mixed: queue.PriorityQueue[object] = queue.PriorityQueue()
mixed.put(1)
try:
    mixed.put("x")
except Exception as exc:
    print(type(exc).__name__)
