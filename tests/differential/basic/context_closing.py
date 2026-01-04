import contextlib


def inner():
    with contextlib.closing(7) as value:
        print(value)
        return value + 1


print(inner())
