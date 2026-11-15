"""A list store that changes how many items are heap objects must keep the
list's exact pointer-item count: later removals rely on it."""


class Box:
    def __init__(self, value):
        self.value = value


def churn(n):
    xs = [0, 1, 2, 3, 4, 5]
    for i in range(n):
        xs[i % 6] = Box(i)  # an object over an int
        xs[(i + 3) % 6] = i  # an int over an object
    total = 0
    while xs:
        item = xs.pop(0)
        total += item.value if isinstance(item, Box) else item
    return total


def mixed():
    ys = ["a", 2, None, 3.5]
    ys[1] = Box(7)
    ys[0] = 1
    ys[2] = "z"
    out = [ys.pop(0), ys.pop().__class__.__name__, ys.pop(0).value, ys.pop()]
    return out, ys


print(churn(1))
print(churn(17))
print(mixed())
