from __future__ import annotations

nums: list[int] = list(range(1_000_000))
total: int = 0
for x in nums:
    total = total + x

print(total)
