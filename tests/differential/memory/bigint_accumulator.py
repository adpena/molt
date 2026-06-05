# RC drop-insertion regression (design 20): a BigInt accumulator loop.
#
# `total` starts as a heap BigInt (1<<60 is past the inline-int window) and each
# `+ 1` / `- 1` allocates a fresh BigInt temporary. Before drop insertion, every
# iteration leaked one or more BigInts and RSS grew without bound (observed
# 3,000,635 allocations, 0 deallocations, 297 MB at exit on the 1M-iter form).
# With drop insertion the previous iteration's BigInt is freed on the back-edge,
# so at most O(1) BigInts are alive and RSS is bounded regardless of n.
#
# Run under `safe_run.py --rss-mb 10`: a leak trips the RSS cap (exit 137) or the
# MOLT_ASSERT_NO_LEAK gate; a correct build prints the final value and exits 0.
def accumulate(n):
    total = 1 << 60
    i = 0
    while i < n:
        total = total + 1
        total = total - 1
        total = total + 1
        i = i + 1
    return total


print(accumulate(1000))
