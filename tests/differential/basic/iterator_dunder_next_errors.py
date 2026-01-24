"""Purpose: differential coverage for __next__ errors and StopIteration."""

class BadIter:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


class ErrorIter:
    def __iter__(self):
        return self

    def __next__(self):
        raise RuntimeError("boom")


if __name__ == "__main__":
    it = iter(BadIter())
    try:
        next(it)
        print("stop", "missed")
    except StopIteration:
        print("stop", "ok")

    it2 = iter(ErrorIter())
    try:
        next(it2)
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)
