"""Purpose: differential coverage for functools.reduce edge cases."""

from functools import reduce


def add(a, b):
    return a + b


if __name__ == "__main__":
    print("basic", reduce(add, [1, 2, 3]))
    print("init", reduce(add, [], 5))
    try:
        reduce(add, [])
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)
