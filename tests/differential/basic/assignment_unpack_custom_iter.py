"""Purpose: differential coverage for unpacking custom iterables."""

class Counter:
    def __init__(self):
        self.count = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.count += 1
        if self.count == 1:
            return "a"
        if self.count == 2:
            return "b"
        raise StopIteration


if __name__ == "__main__":
    counter = Counter()
    x, y = counter
    print("values", x, y)
    print("count", counter.count)
