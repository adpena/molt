"""Purpose: differential coverage for functools.partialmethod."""

from functools import partialmethod


class Counter:
    def __init__(self, base):
        self.base = base

    def add(self, value, delta):
        return self.base + value + delta

    add5 = partialmethod(add, 5)


def main():
    counter = Counter(10)
    print("add5", counter.add5(7))
    bound = Counter.add5.__get__(counter, Counter)
    print("bound", bound(3))


if __name__ == "__main__":
    main()
