"""Test that collections.deque works correctly when shared across threads.

Verifies cross-thread visibility of deque mutations: append, popleft,
extend, and len(). All reads happen after join() to keep output deterministic.
"""

import threading
from collections import deque

# Test 1: Main creates, worker appends, main reads
d1 = deque()

def worker1():
    for i in range(5):
        d1.append(i)

t = threading.Thread(target=worker1)
t.start()
t.join()
print("test1_worker_append:", list(d1))

# Test 2: Main creates with items, worker poplefts
d2 = deque([10, 20, 30])
results2 = []

def worker2():
    results2.append(d2.popleft())
    results2.append(d2.popleft())

t = threading.Thread(target=worker2)
t.start()
t.join()
print("test2_worker_popleft:", results2, list(d2))

# Test 3: Multiple workers append to the same deque concurrently
d3 = deque()

def worker3(offset):
    for i in range(4):
        d3.append(offset + i)

threads = []
for base in (0, 100, 200):
    t = threading.Thread(target=worker3, args=(base,))
    threads.append(t)
    t.start()
for t in threads:
    t.join()
print("test3_concurrent_append:", sorted(d3))

# Test 4: Worker thread creates deque, main thread reads it
container = {}

def worker4():
    container["d"] = deque([7, 8, 9])

t = threading.Thread(target=worker4)
t.start()
t.join()
print("test4_worker_creates:", list(container["d"]))

# Test 5: deque.extend() from worker thread
d5 = deque([1])

def worker5():
    d5.extend([2, 3, 4])

t = threading.Thread(target=worker5)
t.start()
t.join()
print("test5_worker_extend:", list(d5))

# Test 6: len() visibility across threads
d6 = deque()
lengths = []

def worker6():
    for i in range(3):
        d6.append(i)
    lengths.append(len(d6))

t = threading.Thread(target=worker6)
t.start()
t.join()
print("test6_len_visibility:", lengths[0], len(d6))
