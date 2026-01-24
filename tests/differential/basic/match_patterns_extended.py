"""Purpose: differential coverage for structural pattern matching variants."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


value = Point(1, 2)
match value:
    case Point(x, y) if x < y:
        print("class_guard", x, y)
    case _:
        print("class_guard", "miss")

seq = [1, 2, 3]
match seq:
    case [1, *rest]:
        print("seq", rest)
    case _:
        print("seq", "miss")

item = {"kind": "ok", "value": 42}
match item:
    case {"kind": "ok", "value": v} | {"kind": "alt", "value": v}:
        print("or", v)
    case _:
        print("or", "miss")

match (1, 2):
    case (a, b) as pair:
        print("as", a, b, pair)
    case _:
        print("as", "miss")
