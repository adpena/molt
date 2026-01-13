import operator


class Inner:
    def __init__(self, y):
        self.y = y


class Obj:
    def __init__(self, x, y):
        self.x = x
        self.inner = Inner(y)


class Box:
    def __init__(self):
        self.val = 9

    def f(self, a, b=0, *, c=0):
        return (self.val, a, b, c)


print(operator.itemgetter(1)([0, 1, 2]))
print(operator.itemgetter(0, 2)([0, 1, 2]))

obj = Obj(3, 4)
print(operator.attrgetter("x")(obj))
print(operator.attrgetter("inner.y")(obj))
print(operator.attrgetter("x", "inner.y")(obj))

box = Box()
print(operator.methodcaller("f", 1, c=2)(box))
