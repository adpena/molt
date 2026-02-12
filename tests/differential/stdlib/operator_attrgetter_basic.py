"""Purpose: differential coverage for operator.attrgetter."""

import operator


class Demo:
    def __init__(self):
        self.value = 3


if __name__ == "__main__":
    getter = operator.attrgetter("value")
    print("value", getter(Demo()))
