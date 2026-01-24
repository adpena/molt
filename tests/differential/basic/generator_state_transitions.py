"""Purpose: differential coverage for generator state transitions."""

import inspect


def gen():
    yield "start"
    yield "end"


if __name__ == "__main__":
    iterator = gen()
    print("state0", inspect.getgeneratorstate(iterator))
    print("first", next(iterator))
    print("state1", inspect.getgeneratorstate(iterator))
    print("second", next(iterator))
    print("state2", inspect.getgeneratorstate(iterator))
    try:
        next(iterator)
    except StopIteration:
        pass
    print("state3", inspect.getgeneratorstate(iterator))
