"""Purpose: differential coverage for PEP 634/635/636 match semantics."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


class Weird:
    __match_args__ = ("b",)

    def __init__(self):
        self.a = 10
        self.b = 20


def guard_log() -> list:
    log = []

    def side(tag):
        log.append(tag)
        return True

    value = Point(1, 2)
    match value:
        case Point(x, y) if side("guard") and x == 1:
            log.append(("point", x, y))
        case _:
            log.append("miss")
    return log


def match_samples() -> None:
    samples = [Point(1, 3), Weird(), {"x": 5, "y": 6}]
    for item in samples:
        match item:
            case Point(1, y):
                print("point", y)
            case Weird(b=val):
                print("weird", val)
            case {"x": x, **rest}:
                print("mapping", x, sorted(rest.items()))
            case _:
                print("other")


match_samples()
print(guard_log())
