"""Purpose: differential coverage for bisect basic."""

import bisect


data = [1, 2, 2, 3]
print(bisect.bisect_left(data, 2))
print(bisect.bisect_right(data, 2))
print(bisect.bisect(data, 2))
print(bisect.bisect_left(data, 2, 2))
print(bisect.bisect_right(data, 2, 2))

try:
    bisect.bisect_left(data, 2, -1)
except Exception as exc:
    print(type(exc).__name__, exc)

try:
    bisect.bisect_left(data, 2, 1.2)
except Exception as exc:
    print(type(exc).__name__, exc)


class BadIndex:
    def __index__(self):
        return 1.5


try:
    bisect.bisect_left(data, 2, BadIndex())
except Exception as exc:
    print(type(exc).__name__, exc)

try:
    bisect.bisect_left(data, 2, 0, 10)
except Exception as exc:
    print(type(exc).__name__, exc)

items = [{"x": 1}, {"x": 2}, {"x": 2}, {"x": 3}]
print(bisect.bisect_left(items, 2, key=lambda d: d["x"]))
print(bisect.bisect_right(items, 2, key=lambda d: d["x"]))

vals = [{"x": 1}, {"x": 3}]
bisect.insort_left(vals, {"x": 2}, key=lambda d: d["x"])
print([val["x"] for val in vals])

vals = [{"x": 1}, {"x": 2}, {"x": 2}, {"x": 3}]
bisect.insort_right(vals, {"x": 2}, key=lambda d: d["x"])
print([val["x"] for val in vals])

nums = [1, 3]
bisect.insort(nums, 2)
print(nums)
