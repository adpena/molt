import math


class IntOnly:
    def __int__(self) -> int:
        return 7


class IndexOnly:
    def __index__(self) -> int:
        return 9


class BadInt:
    def __int__(self):
        return "x"


class Roundy:
    def __round__(self, ndigits=None):
        return 123 if ndigits is None else 456


class Truncy:
    def __trunc__(self):
        return 11


class BadTrunc:
    def __trunc__(self):
        return "x"


def main() -> None:
    print(int(IntOnly()))
    print(int(IndexOnly()))
    try:
        int(BadInt())
    except TypeError:
        print("TypeError")
    print(round(Roundy()))
    print(round(Roundy(), 2))
    print(math.trunc(Truncy()))
    print(math.trunc(BadTrunc()))
    print(int("  123 "))
    print(int(b"11", 2))
    print(int("0x10", 0))
    print(int(True))
    try:
        int(1.2, 10)
    except TypeError:
        print("TypeError")
    try:
        int("x", 2)
    except ValueError:
        print("ValueError")
    try:
        int(float("nan"))
    except ValueError:
        print("ValueError")
    try:
        int(float("inf"))
    except OverflowError:
        print("OverflowError")


if __name__ == "__main__":
    main()
