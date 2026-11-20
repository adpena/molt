"""Field reads and stores through a parameter that holds an exact instance.

A function whose every caller passes an instance one class statement built
reads and writes that instance's fields as CPython does: values, stores,
a field that was never set (AttributeError), a parameter that also receives a
subclass instance, and a class whose namespace is mutated after creation.
"""


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


class Sub(Point):
    pass


def norm2(point):
    return point.x * point.x + point.y * point.y


def move(point, dx):
    point.x = point.x + dx
    return point.x


def either(point):
    return point.x + point.y


class Lazy:
    def __init__(self, flag):
        if flag:
            self.value = 1


def read_value(item):
    return item.value


class Mutated:
    def __init__(self):
        self.a = 1


Mutated.extra = 2


def read_a(item):
    return item.a + item.extra


total = 0
for index in range(1000):
    total += norm2(Point(index, index + 1))
print("norm2 total", total)
moved = Point(1, 2)
print("move", move(moved, 5), moved.x, moved.y)
print("either", either(Point(3, 4)), either(Sub(5, 6)))
try:
    read_value(Lazy(False))
except AttributeError as exc:
    print("missing", exc)
print("present", read_value(Lazy(True)))
print("mutated", read_a(Mutated()))
