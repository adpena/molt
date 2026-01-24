"""Purpose: differential coverage for __iter__ returning non-iterator."""

class BadIter:
    def __iter__(self):
        return 123


if __name__ == "__main__":
    try:
        iter(BadIter())
        print("iter", "missed")
    except Exception as exc:
        print("iter", type(exc).__name__)
