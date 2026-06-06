"""Purpose: differential coverage for queue operations under concurrency."""

import threading
import queue


# 1. Basic Queue put/get
q = queue.Queue()
q.put("hello")
q.put("world")
print("get-1", q.get())
print("get-2", q.get())


# 2. Queue size
q = queue.Queue()
print("empty", q.empty())
q.put(1)
q.put(2)
print("size", q.qsize())
print("empty", q.empty())


# 3. Queue maxsize
q = queue.Queue(maxsize=2)
q.put("a")
q.put("b")
print("full", q.full())
q.get()
print("full-after-get", q.full())


# 4. Queue.get with timeout on empty queue
q = queue.Queue()
try:
    q.get(timeout=0.01)
except queue.Empty:
    print("empty-timeout")


# 5. Queue.put with timeout on full queue
q = queue.Queue(maxsize=1)
q.put("x")
try:
    q.put("y", timeout=0.01)
except queue.Full:
    print("full-timeout")


# 6. get_nowait on empty
q = queue.Queue()
try:
    q.get_nowait()
except queue.Empty:
    print("get-nowait-empty")


# 7. put_nowait on full
q = queue.Queue(maxsize=1)
q.put("x")
try:
    q.put_nowait("y")
except queue.Full:
    print("put-nowait-full")


# 8. Producer-consumer pattern
q = queue.Queue()
results = []

def producer(q, items):
    for item in items:
        q.put(item)

def consumer(q, results, count):
    for _ in range(count):
        results.append(q.get())

items = list(range(10))
p = threading.Thread(target=producer, args=(q, items))
c = threading.Thread(target=consumer, args=(q, results, 10))
p.start()
c.start()
p.join()
c.join()
print("produced-consumed", sorted(results))


# 9. Multiple producers, single consumer
q = queue.Queue()
results = []

def producer_n(q, start, count):
    for i in range(start, start + count):
        q.put(i)

total = 20
threads = []
for i in range(4):
    t = threading.Thread(target=producer_n, args=(q, i * 5, 5))
    threads.append(t)
    t.start()

c = threading.Thread(target=consumer, args=(q, results, total))
c.start()

for t in threads:
    t.join()
c.join()

print("multi-producer", len(results), sorted(results))


# 10. LifoQueue (stack)
lq = queue.LifoQueue()
lq.put(1)
lq.put(2)
lq.put(3)
print("lifo", lq.get(), lq.get(), lq.get())


# 11. PriorityQueue
pq = queue.PriorityQueue()
pq.put((3, "low"))
pq.put((1, "high"))
pq.put((2, "medium"))
print("prio", pq.get()[1], pq.get()[1], pq.get()[1])


# 12. task_done and join
q = queue.Queue()
q.put("task1")
q.put("task2")

def task_worker(q):
    while True:
        try:
            item = q.get_nowait()
        except queue.Empty:
            break
        q.task_done()

t = threading.Thread(target=task_worker, args=(q,))
t.start()
t.join()
q.join()
print("all-tasks-done")
