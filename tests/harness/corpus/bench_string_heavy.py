# Benchmark: heavy string operations that benefit from ASCII interning.
#
# Character-by-character iteration over a string produces single-char
# substrings.  With ASCII interning, these should be immortal singletons
# (no allocation, no refcount traffic).  Without interning, each char
# is a fresh heap allocation that must be refcounted and freed.
#
# Key metrics (MOLT_PROFILE=1):
#   alloc_string — should be very low with interning (just the source
#       string + the output), high without (50_000 * len(data) chars).
#   alloc_bytes_string — total string payload bytes allocated.

# JSON-like string processing
data = '{"key": "value", "count": 42, "items": ["a", "b", "c"]}'
for _ in range(50_000):
    # Character-by-character processing
    result = []
    for ch in data:
        if ch == '"':
            result.append("'")
        else:
            result.append(ch)
    output = "".join(result)

print(f"len={len(output)}")
