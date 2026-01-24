"""Purpose: differential coverage for return in finally within generator."""


def gen():
    try:
        yield "start"
        return "value"
    finally:
        return "finally"


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    try:
        next(iterator)
        print("missing", "stop")
    except StopIteration as exc:
        print("stop", exc.value)
