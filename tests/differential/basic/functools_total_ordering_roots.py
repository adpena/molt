"""Purpose: differential coverage for functools.total_ordering root variants."""

from functools import total_ordering


@total_ordering
class Lt:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __lt__(self, other):
        return self.value < other.value


@total_ordering
class Le:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __le__(self, other):
        return self.value <= other.value


@total_ordering
class Gt:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __gt__(self, other):
        return self.value > other.value


@total_ordering
class Ge:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __ge__(self, other):
        return self.value >= other.value


for label, cls in (("lt", Lt), ("le", Le), ("gt", Gt), ("ge", Ge)):
    low = cls(1)
    high = cls(2)
    print(label, low < high, low <= high, low > high, low >= high)


@total_ordering
class Preserve:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __lt__(self, other):
        return self.value < other.value

    def __le__(self, other):
        return "custom-le"


left = Preserve(1)
right = Preserve(2)
# Verify total_ordering did not replace an explicitly provided __le__.
print("preserve_le", left.__le__(right))
print("preserve_gt", right > left)
