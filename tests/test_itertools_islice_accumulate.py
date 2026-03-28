"""Reproducer: itertools.islice and itertools.accumulate return empty lists."""
import itertools

# islice basic
result = list(itertools.islice(range(100), 5))
print("islice_basic:", result)
assert result == [0, 1, 2, 3, 4], f"Expected [0,1,2,3,4] got {result}"

# accumulate basic
result2 = list(itertools.accumulate([1, 2, 3, 4]))
print("accumulate_basic:", result2)
assert result2 == [1, 3, 6, 10], f"Expected [1,3,6,10] got {result2}"

# islice with start/stop
result3 = list(itertools.islice(range(10), 2, 5))
print("islice_start_stop:", result3)
assert result3 == [2, 3, 4], f"Expected [2,3,4] got {result3}"

# islice with step
result4 = list(itertools.islice(range(10), 0, 10, 3))
print("islice_step:", result4)
assert result4 == [0, 3, 6, 9], f"Expected [0,3,6,9] got {result4}"

print("ALL PASSED")
