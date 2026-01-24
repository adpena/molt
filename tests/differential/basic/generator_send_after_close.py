"""Purpose: differential coverage for send after close."""


def gen():
    yield 1


if __name__ == "__main__":
    iterator = gen()
    print("first", next(iterator))
    iterator.close()
    try:
        iterator.send(None)
        print("send", "missed")
    except Exception as exc:
        print("send", type(exc).__name__)
