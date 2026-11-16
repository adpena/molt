"""A module whose globals change after initialization."""

counter = 0


def bump() -> int:
    global counter
    counter += 1
    return counter
