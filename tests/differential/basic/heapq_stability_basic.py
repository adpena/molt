"""Purpose: differential coverage for heapq stability and tie-breaking."""

import heapq

items = [(1, "b"), (1, "a"), (2, "c")]
heap = items[:]
heapq.heapify(heap)
print(heapq.heappop(heap))
print(heapq.heappop(heap))
