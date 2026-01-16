import heapq


data = [3, 1, 4, 1, 5]
heapq.heapify(data)
print(data)
heapq.heappush(data, 0)
print(data)
print(heapq.heappop(data))
print(data)
print(heapq.heappushpop(data, 2))
print(data)
print(heapq.heapreplace(data, 6))
print(data)
print(heapq.heappushpop([], 4))

try:
    heapq.heappop([])
except Exception as exc:
    print(type(exc).__name__, exc)

try:
    heapq.heapreplace([], 1)
except Exception as exc:
    print(type(exc).__name__, exc)

print(heapq.nsmallest(3, [5, 1, 3, 2, 4]))
print(heapq.nlargest(2, [5, 1, 3, 2, 4]))

items = [{"x": 2}, {"x": 1}, {"x": 3}]
print([item["x"] for item in heapq.nsmallest(2, items, key=lambda d: d["x"])])
print([item["x"] for item in heapq.nlargest(2, items, key=lambda d: d["x"])])
