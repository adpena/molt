"""Purpose: differential coverage for iterator exhaustion behavior."""

class Once:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


if __name__ == "__main__":
    it = Once()
    for _ in range(2):
        try:
            next(it)
            print("stop", "missed")
        except StopIteration:
            print("stop", "ok")
