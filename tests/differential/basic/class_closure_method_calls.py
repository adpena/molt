"""Proven method calls into closure targets (design 78, 5.10).

A method that uses zero-argument super() or names __class__ holds the class
cell; a method of a class inside a function holds that function's cells. A
call on an exact receiver enters it with its function's live closure tuple.
"""


class Base:
    def compute(self, x):
        return x + 1

    def name(self):
        return "Base"


class Mid(Base):
    def compute(self, x):
        return super().compute(x) * 3

    def name(self):
        return "Mid<" + super().name() + ">"


class Leaf(Mid):
    def compute(self, x):
        return super().compute(x) - 2

    def name(self):
        return "Leaf<" + super().name() + ">"

    def owner(self):
        return __class__.__name__


def hierarchy():
    obj = Leaf()
    total = 0
    for i in range(1000):
        total += obj.compute(i)
    print("hierarchy", total, obj.name(), obj.owner())


def make_counter(start):
    count = start

    class Counter:
        def bump(self, step):
            nonlocal count
            count += step
            return count

        def peek(self):
            return count

    return Counter()


def enclosing_cells():
    counter = make_counter(10)
    for step in range(5):
        counter.bump(step)
    print("enclosing", counter.peek(), counter.bump(100))
    other = make_counter(-1)
    print("separate", other.peek(), counter.peek())


class A:
    def who(self):
        return ["A"]


class B(A):
    def who(self):
        return ["B"] + super().who()


class C(A):
    def who(self):
        return ["C"] + super().who()


class D(B, C):
    def who(self):
        return ["D"] + super().who()


def diamond():
    d = D()
    print("diamond", d.who(), [k.__name__ for k in D.__mro__])


class Recursive:
    def depth(self, n):
        if n == 0:
            return __class__.__name__
        return self.depth(n - 1)


def recursion():
    print("recursion", Recursive().depth(50))


class Raising(Base):
    def compute(self, x):
        if x < 0:
            raise ValueError(f"negative {x} in {__class__.__name__}")
        return super().compute(x)


def exceptions():
    r = Raising()
    print("raising ok", r.compute(4))
    try:
        r.compute(-3)
    except ValueError as exc:
        print("raising", exc)


hierarchy()
enclosing_cells()
diamond()
recursion()
exceptions()


class Named:
    @property
    def label(self):
        return "named"


class Labelled(Named):
    @property
    def label(self):
        return "labelled<" + super().label + ">"

    @staticmethod
    def tag():
        return __class__.__name__


def properties():
    obj = Labelled()
    print("property", obj.label, Labelled.tag(), obj.tag())


properties()
