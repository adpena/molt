"""Purpose: differential coverage for comparison fallback on NotImplemented."""

log = []


class Left:
    def __lt__(self, other):
        log.append("left_lt")
        return NotImplemented


class Right:
    def __gt__(self, other):
        log.append("right_gt")
        return True


if __name__ == "__main__":
    result = Left() < Right()
    print("result", result)
    print("log", log)
