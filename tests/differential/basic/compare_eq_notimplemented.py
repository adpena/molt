"""Purpose: differential coverage for __eq__ NotImplemented behavior."""

log = []


class Left:
    def __eq__(self, other):
        log.append("left_eq")
        return NotImplemented


class Right:
    def __eq__(self, other):
        log.append("right_eq")
        return False


if __name__ == "__main__":
    result = Left() == Right()
    print("result", result)
    print("log", log)
