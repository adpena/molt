"""Purpose: a class whose only special members are a `property` and plain
methods stays provable (HF-847): `property(fget)` runs no code when built, an
instance load calls the getter, and an operator on a value that may be an
instance runs only that instance's special methods. Each block separates the
correct lowering from a plausible defect: a getter skipped or called on the
wrong object, a data descriptor shadowed by an instance dictionary, a setter
or deleter bypassed, a subclass property ignored, a getter whose side effect
is lost, and a reflected operator on a returned instance not run."""

calls = []


# `@v.setter` calls an attribute of the property object, which the analysis
# cannot follow; that call can reach the module globals bound before it, so
# the provable classes are defined after it.
class Settable:
    def __init__(self, v):
        self._v = v

    @property
    def v(self):
        return self._v

    @v.setter
    def v(self, value):
        calls.append(("set", value))
        self._v = value * 10

    @v.deleter
    def v(self):
        calls.append(("del",))
        self._v = -1


class Box:
    def __init__(self, x):
        self._x = x

    @property
    def x(self) -> int:
        calls.append(("get", self._x))
        return self._x

    def twice(self):
        return self.x * 2


class Base:
    def __init__(self, x):
        self._x = x

    @property
    def x(self):
        return self._x


class Child(Base):
    @property
    def x(self):
        return 100 + self._x


class Reflected:
    def __init__(self, x):
        self._x = x

    @property
    def x(self):
        return self._x


counter = 0


class Counting:
    def __init__(self):
        self.n = 0

    @property
    def bump(self):
        global counter
        counter += 1
        self.n += 1
        return self.n


class Adder:
    """A method returns the instance; `+=` must run its reflected add."""

    def __init__(self, k):
        self.k = k

    def me(self):
        return self

    def __radd__(self, other):
        return other + self.k


def main():
    box = Box(3)
    total = 0
    i = 0
    while i < 4:
        total += box.x
        i += 1
    print("box", total, box.twice(), len(calls))

    # A property is a data descriptor: an instance dictionary cannot shadow it.
    r = Reflected(4)
    r.__dict__["x"] = 999
    print("shadow", r.x, r.__dict__["x"])

    # The class attribute is the property object itself.
    prop = Reflected.x
    print("class", type(prop).__name__, prop.fget(r), Reflected.x is prop)

    s = Settable(2)
    s.v = 5
    print("setter", s.v, s._v)
    del s.v
    print("deleter", s.v)

    # A subclass that overrides the property.
    for obj in (Base(1), Child(1)):
        print("sub", type(obj).__name__, obj.x)

    c = Counting()
    a = c.bump
    b = c.bump
    print("side", a, b, counter, c.n)

    acc = 1
    adder = Adder(7)
    for _ in range(3):
        acc += adder.me()
    print("radd", acc)

    print("calls", calls[-3:])


main()
