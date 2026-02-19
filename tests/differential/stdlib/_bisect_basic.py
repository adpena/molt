"""Purpose: differential coverage for _bisect basics."""

import _bisect


data = [1, 2, 2, 3]
print(_bisect.bisect_left(data, 2))
print(_bisect.bisect_right(data, 2))
print(_bisect.bisect(data, 2))

nums = [1, 3]
_bisect.insort(nums, 2)
print(nums)

items = [{"x": 1}, {"x": 2}, {"x": 2}, {"x": 3}]
print(_bisect.bisect_left(items, 2, key=lambda d: d["x"]))

vals = [{"x": 1}, {"x": 3}]
_bisect.insort_left(vals, {"x": 2}, key=lambda d: d["x"])
print([val["x"] for val in vals])
