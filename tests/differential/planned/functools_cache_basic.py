"""Purpose: differential coverage for functools.cache."""

from functools import cache

calls = 0


@cache
def double(value):
    global calls
    calls += 1
    return value * 2


def main():
    print("first", double(3))
    print("second", double(3))
    print("third", double(4))
    info = double.cache_info()
    print("info", info.hits, info.misses)
    print("calls", calls)
    double.cache_clear()
    print("after_clear", double(3), double.cache_info().hits)


if __name__ == "__main__":
    main()
