# Benchmark: measures refcount cleanup speed for primitive-only containers.
# Lists/dicts/tuples of ints should have HEADER_FLAG_CONTAINS_REFS=0,
# so dec_ref skips element-by-element iteration entirely.
#
# Key metrics (MOLT_PROFILE=1):
#   alloc_tuple, alloc_dict — total container allocations
#   Compare wall-clock time vs. a version using string elements (which
#   must iterate on cleanup).

# Create and destroy many lists of ints (no heap refs)
for _ in range(100_000):
    data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

# Create and destroy many dicts of int keys/values
for _ in range(100_000):
    d = {1: 10, 2: 20, 3: 30, 4: 40, 5: 50}

# Create and destroy many tuples of ints
for _ in range(100_000):
    t = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10)

print("done")
