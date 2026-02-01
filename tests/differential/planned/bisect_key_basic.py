"""Purpose: differential coverage for bisect key support."""

import bisect

items = [{"v": 1}, {"v": 3}, {"v": 5}]
idx = bisect.bisect_left(items, {"v": 4}, key=lambda x: x["v"])
print(idx)
