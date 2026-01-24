"""Purpose: differential coverage for StopIteration value from generator."""


def gen():
    yield 1
    return "done"


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    try:
        next(iterator)
        print("missing", "stop")
    except StopIteration as exc:
        print("stop", exc.value)
