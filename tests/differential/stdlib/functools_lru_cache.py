from functools import lru_cache

@lru_cache(maxsize=32)
def fib(n):
    if n < 2:
        return n
    return fib(n-1) + fib(n-2)

for i in range(15):
    print(fib(i))

info = fib.cache_info()
print(f"hits={info.hits}, misses={info.misses}")
