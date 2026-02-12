"""Purpose: differential coverage for functools.lru_cache eviction and cache_info."""

from functools import lru_cache


@lru_cache(maxsize=2)
def f(x):
    return x * 10


print("v", f(1), f(2), f(3))
print("v2", f(2), f(1))
info = f.cache_info()
print("info", info.hits, info.misses, info.maxsize, info.currsize)
