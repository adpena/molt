"""Purpose: differential coverage for unpacking errors with custom iterables."""

class Counter:
    def __iter__(self):
        return self

    def __next__(self):
        return "x"


if __name__ == "__main__":
    try:
        a, b = Counter()
        print("missing", "error")
    except Exception as exc:
        print("error", type(exc).__name__)
