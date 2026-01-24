"""Purpose: differential coverage for kw-only default evaluation timing."""

log = []


def default():
    log.append("default")
    return 3


def f(*, x=default()):
    return x


if __name__ == "__main__":
    print("log", log)
    print("call", f())
    print("log_after", log)
