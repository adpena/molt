"""Purpose: differential coverage for functools.cache behavior."""

from functools import cache

calls = 0


@cache
def f(value):
    global calls
    calls += 1
    return value * 2


if __name__ == "__main__":
    print("first", f(2))
    print("second", f(2))
    print("calls", calls)
    info = f.cache_info()
    print("info", info.hits, info.misses)
