# RC drop-insertion regression (design 20): a list comprehension.
#
# `[x * 2 for x in range(10000)]` materializes 10,000 int elements. The elements
# below the inline-int window are unboxed (no heap), but the comprehension still
# exercises the iterator-temporary and result-list ownership paths: the iterator,
# the per-iteration loop variable, and the multiplication temporaries must all be
# freed. The list itself is alive until the `sum`/`len` consumes it, then freed.
# A leak in any of these temporaries grows RSS with the iteration count.
#
# Run under `safe_run.py --rss-mb 5`.
def build_and_sum(n):
    xs = [x * 2 for x in range(n)]
    return len(xs), sum(xs)


print(build_and_sum(10000))
