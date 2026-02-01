"""Purpose: differential coverage for heapq nlargest/nsmallest."""

import heapq

items = [5, 1, 3, 2, 4]
print(heapq.nlargest(2, items))
print(heapq.nsmallest(2, items))
