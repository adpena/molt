"""Purpose: differential coverage for functools.total_ordering."""

from functools import total_ordering


@total_ordering
class Item:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __lt__(self, other):
        return self.value < other.value


if __name__ == "__main__":
    a = Item(1)
    b = Item(2)
    print("lt", a < b)
    print("le", a <= b)
    print("gt", b > a)
