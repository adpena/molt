"""Purpose: differential coverage for f-string evaluation order."""

log = []


def side(tag):
    log.append(tag)
    return tag


if __name__ == "__main__":
    value = f"{side('a')}{side('b')}{side('c')}"
    print("value", value)
    print("log", log)
