"""Purpose: differential coverage for sending non-None into new generator."""


def gen():
    yield "start"


if __name__ == "__main__":
    iterator = gen()
    try:
        iterator.send(1)
        print("send", "missed")
    except Exception as exc:
        print("send", type(exc).__name__)
