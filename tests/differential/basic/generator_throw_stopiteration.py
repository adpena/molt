"""Purpose: differential coverage for throwing StopIteration into generator."""


def gen():
    yield 1


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    try:
        iterator.throw(StopIteration("stop"))
        print("throw", "missed")
    except Exception as exc:
        print("throw", type(exc).__name__)
