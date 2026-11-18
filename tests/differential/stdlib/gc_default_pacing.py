"""Purpose: the collector starts with the target version's default pacing.

CPython 3.12 starts with thresholds (700, 10, 10) and 3.13+ with
(2000, 10, 10). The young-collection count over a fixed allocation run must
follow the default threshold, not only the value gc.get_threshold() reports.
"""

import gc

thresholds = gc.get_threshold()
print("default thresholds", thresholds)

young = [0, 0, 0]


def count_collections(phase, info):
    if phase == "stop":
        young[info["generation"]] += 1


gc.collect()
gc.callbacks.append(count_collections)
rows = []
for i in range(100_000):
    rows.append({"key": i, "value": [i, i + 1, i + 2]})
gc.callbacks.remove(count_collections)

# 200,000 container allocations: about 200,000 / threshold young collections
# (CPython 3.15: 90 at 2000; CPython 3.12: 260 at 700).
allocations = 2 * len(rows)
expected = allocations // thresholds[0]
print("young collections follow the threshold", abs(young[0] - expected) * 5 <= expected)
print("older collections ran", young[1] > 0)
