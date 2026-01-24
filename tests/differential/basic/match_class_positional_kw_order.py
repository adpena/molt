"""Purpose: differential coverage for class pattern positional/kw order."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


value = Point(1, 2)
match value:
    case Point(1, y=2):
        print("ok", 1, 2)
    case _:
        print("ok", "miss")

try:
    match value:
        case Point(1, x=1):
            print("dup", "hit")
        case _:
            print("dup", "miss")
except Exception as exc:
    print("dup", type(exc).__name__)
