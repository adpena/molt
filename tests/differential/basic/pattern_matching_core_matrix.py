"""Purpose: differential coverage for core pattern matching cases."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y) -> None:
        self.x = x
        self.y = y


def classify(value):
    match value:
        case 0:
            return "zero"
        case [a, b]:
            return f"seq:{a},{b}"
        case {"k": v}:
            return f"map:{v}"
        case Point(x=1, y=y):
            return f"point:{y}"
        case 1 | 2 as v:
            return f"or:{v}"
        case _ if isinstance(value, str):
            return "str"
        case _ as other:
            return f"other:{other}"


items = [
    0,
    [1, 2],
    {"k": 9},
    Point(1, 4),
    2,
    "hi",
    99,
]

print([classify(item) for item in items])
