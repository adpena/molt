# RC drop-insertion regression (design 20): a string-concat loop.
#
# Each `s + "x"` allocates a fresh string object; the previous `s` is dead after
# the reassignment and must be freed. Before drop insertion this loop leaked one
# string per iteration (30M-iter form OOM'd at a 512 MB cap). The accumulating
# result string itself grows to ~n bytes, so RSS is bounded by the final string
# length plus O(1) live temporaries, not by the iteration count's worth of
# intermediate strings.
#
# Run under `safe_run.py --rss-mb 20`: a per-iteration leak trips the cap.
def concat(n):
    s = ""
    i = 0
    while i < n:
        s = s + "x"
        i = i + 1
    return len(s)


print(concat(10000))
