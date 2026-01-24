"""Purpose: differential coverage for boolean short-circuit order."""

log = []


def side(tag, value):
    log.append(tag)
    return value


if __name__ == "__main__":
    result_and = side("a", False) and side("b", True)
    result_or = side("c", True) or side("d", False)
    print("and", result_and)
    print("or", result_or)
    print("log", log)
