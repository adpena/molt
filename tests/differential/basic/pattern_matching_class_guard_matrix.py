"""Purpose: differential coverage for class patterns + guards matrix."""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


class Box:
    __match_args__ = ("value",)

    def __init__(self, value):
        self.value = value


def guard_log() -> list:
    log = []

    def guard(tag, result=True):
        log.append(tag)
        return result

    sample = Box(Point(1, 2))
    match sample:
        case Box(Point(x, y)) if guard("g1") and x == 1:
            log.append(("match", x, y))
        case _:
            log.append("miss")
    return log


def run() -> None:
    samples = [Point(1, 2), Point(3, 4), Box(10)]
    for item in samples:
        match item:
            case Point(1, y):
                print("point", y)
            case Box(value=v) if v > 5:
                print("box", v)
            case _:
                print("other")
    print(guard_log())


run()
