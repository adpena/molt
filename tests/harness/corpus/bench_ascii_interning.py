# Benchmark: measures string allocation pressure from chr() and iteration.
# Run with MOLT_PROFILE=1 to see allocation counts.
#
# Key metric: alloc_string count.  With ASCII interning, chr(i % 128)
# should reuse pre-allocated immortal objects, so alloc_string stays low.
# Without interning, alloc_string ~ 100_000 (one per chr() call).
#
# Also measures alloc_bytes_string: total bytes allocated for string payloads.
# Interning should keep this near zero for the chr() loop.

total = 0
for i in range(100_000):
    c = chr(i % 128)  # ASCII chars should be interned
    total += len(c)
print(f"total={total}")

# String split on ASCII text - each result char should be interned
text = "hello world this is a test of ascii interning"
for _ in range(10_000):
    parts = list(text)
    total += len(parts)
print(f"total={total}")
