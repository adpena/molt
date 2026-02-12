"""Purpose: differential coverage for bisect edge cases."""

import bisect

arr = [1, 2, 2, 3]
print(bisect.bisect_left(arr, 2))
print(bisect.bisect_right(arr, 2))
