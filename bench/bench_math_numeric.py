"""Matched exact-carrier math workload through the existing bench harness."""

import math

iterations = 300_000
index = 0
total = 0.0
while index < iterations:
    total += math.sqrt(4.0)
    total += math.log2(8)
    total += math.floor(1.25)
    total += math.sqrt(True)
    index += 1
assert total == 7.0 * iterations
print(iterations, total)
