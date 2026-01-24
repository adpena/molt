"""Purpose: differential coverage for list sort."""


def expect_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(type(exc).__name__)


data = [3, 1, 2]
data.sort()
print(data)

data = ["bb", "a", "ccc"]
data.sort(key=len)
print(data)

data = [("b", 2), ("a", 1), ("b", 1)]
data.sort(key=lambda x: x[0])
print(data)

data = [3, 1, 2]
data.sort(reverse=True)
print(data)

expect_error(lambda: [1, 2].sort(None))
expect_error(lambda: [1, "a"].sort())


class Box:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v

    def __gt__(self, other):
        return self.v > other.v

    def __le__(self, other):
        return self.v <= other.v

    def __ge__(self, other):
        return self.v >= other.v


boxes = [Box(2), Box(1)]
boxes.sort()
vals = []
for b in boxes:
    vals.append(b.v)
print(vals)
print(Box(1) < Box(2), Box(1) <= Box(2), Box(1) > Box(2), Box(1) >= Box(2))


class Weird:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return False

    def __le__(self, other):
        return True

    def __gt__(self, other):
        return False

    def __ge__(self, other):
        return False


print(Weird(1) < Weird(2), Weird(1) <= Weird(2), Weird(1) >= Weird(2))


class Boom:
    def __lt__(self, other):
        raise ValueError("boom")


expect_error(lambda: sorted([Boom(), Boom()]))
