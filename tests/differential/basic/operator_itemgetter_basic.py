"""Purpose: differential coverage for operator.itemgetter."""

import operator


if __name__ == "__main__":
    getter = operator.itemgetter(1)
    print("value", getter([0, 1, 2]))
