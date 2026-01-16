import operator


class Inner:
    def __init__(self) -> None:
        self.value = 7


class Obj:
    def __init__(self) -> None:
        self.value = 3
        self.inner = Inner()

    def add(self, x: int, y: int = 0) -> int:
        return self.value + x + y


obj = Obj()

print(operator.add(2, 3))
print(operator.mul(3, 4))
print(operator.eq(3, 3), operator.eq(3, 4))

print(operator.itemgetter(1)([10, 20, 30]))
print(operator.itemgetter(0, 2)([10, 20, 30]))
print(operator.itemgetter(slice(1, None))([1, 2, 3]))

print(operator.attrgetter("value")(obj))
print(operator.attrgetter("inner.value")(obj))
print(operator.attrgetter("value", "inner.value")(obj))

print(operator.methodcaller("add", 5)(obj))
print(operator.methodcaller("add", 5, y=2)(obj))

try:
    operator.itemgetter()
except TypeError:
    print("itemgetter error")

try:
    operator.attrgetter()
except TypeError:
    print("attrgetter error")

try:
    operator.attrgetter("missing")(obj)
except AttributeError:
    print("attrgetter missing")

try:
    operator.itemgetter(5)([1, 2])
except Exception as exc:
    print(type(exc).__name__)
