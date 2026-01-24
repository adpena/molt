"""Purpose: differential coverage for generator close/throw edge cases."""


def gen():
    try:
        yield "start"
        yield "middle"
    finally:
        yield "cleanup"


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    try:
        iterator.close()
        print("close", "done")
    except Exception as exc:
        print("close", type(exc).__name__)

    iterator2 = gen()
    print("first2", next(iterator2))
    try:
        iterator2.throw(ValueError("boom"))
        print("throw", "done")
    except Exception as exc:
        print("throw", type(exc).__name__)
