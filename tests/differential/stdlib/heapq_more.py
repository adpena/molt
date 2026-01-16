import heapq

data = [3, 1, 4, 1, 5, 9, 2]
heapq._heapify_max(data)
out = []
while data:
    out.append(heapq._heappop_max(data))
print(out)

try:
    heapq._heappop_max([])
except IndexError:
    print("empty")

print(list(heapq.merge([1, 3, 5], [2, 4], [0, 6])))
print(list(heapq.merge([5, 3, 1], [6, 4, 2], reverse=True)))

items1 = [{"x": 1}, {"x": 3}]
items2 = [{"x": 2}, {"x": 4}]
merged = heapq.merge(items1, items2, key=lambda d: d["x"])
print([item["x"] for item in merged])
