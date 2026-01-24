"""Purpose: differential coverage for class pattern errors and __match_args__."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


class BadMatch:
    __match_args__ = ("missing",)


value = Point(1, 2)
match value:
    case Point(1, 2):
        print("point", "ok")
    case _:
        print("point", "miss")

try:
    match BadMatch():
        case BadMatch(1):
            print("bad", "hit")
        case _:
            print("bad", "miss")
except Exception as exc:
    print("bad", type(exc).__name__)
