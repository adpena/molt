"""Purpose: differential coverage for functools.lru_cache cache_info/clear."""

import functools

@functools.lru_cache(maxsize=2)
def f(x):
    return x + 1

print(f(1))
print(f(2))
print(f(1))
print(f.cache_info())
f.cache_clear()
print(f.cache_info())
