"""Purpose: differential coverage for heapq.merge with key."""

import heapq

items1 = ["a", "bbb"]
items2 = ["cc"]
print(list(heapq.merge(items1, items2, key=len)))
