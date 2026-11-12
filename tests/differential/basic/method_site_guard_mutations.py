"""Purpose: a compiled method site observes every mutation that changes what
`obj.m(args)` selects at the next execution of the same site (call_anatomy.md
invariant J3): rebinding the method on the class, an instance attribute that
shadows it, `__class__` assignment, a base-class change that a subclass
inherits, a subclass that overrides it, and `__defaults__` / `__code__`
reassignment on the selected function. Each `step` call below runs the one
site in `run`.
"""


class Base:
    def m(self, x, bump=1):
        return ("Base.m", x + bump)


class Leaf(Base):
    pass


class Other:
    def m(self, x, bump=100):
        return ("Other.m", x + bump)


class Slotted:
    __slots__ = ("v",)

    def __init__(self):
        self.v = 0

    def m(self, x, bump=1):
        return ("Slotted.m", x + bump + self.v)


def run(obj, x):
    # The site under test.
    return obj.m(x)


def run_two(obj, x, y):
    return obj.m(x, y)


def warm(obj, label):
    for i in range(50):
        run(obj, i)
    print(label, run(obj, 1), run_two(obj, 1, 2))


def rebound_m(self, x, bump=10):
    return ("rebound", x + bump)


def other_code(self, x, bump=1):
    return ("new code", x * bump)


def recurse(obj, n):
    return obj.down(n)


class Deep:
    def down(self, n):
        if n == 0:
            return 0
        return recurse(self, n - 1) + 1


def main():
    leaf = Leaf()
    warm(leaf, "warm")

    # Rebind the method on the class the site resolved through.
    Base.m = rebound_m
    warm(leaf, "rebound on base")

    # A subclass override added after the site warmed.
    Leaf.m = lambda self, x, bump=3: ("leaf lambda", x + bump)
    warm(leaf, "override on leaf")
    del Leaf.m
    warm(leaf, "override removed")

    # An instance attribute shadows the method, then goes away.
    leaf.m = lambda x, bump=5: ("instance", x + bump)
    warm(leaf, "instance shadow")
    del leaf.m
    warm(leaf, "shadow removed")

    # setattr on the class, the monkeypatch spelling.
    setattr(Base, "m", lambda self, x, bump=7: ("setattr", x + bump))
    warm(leaf, "setattr")

    # __defaults__ and __code__ of the selected function.
    Base.m = rebound_m
    warm(leaf, "defaults before")
    rebound_m.__defaults__ = (1000,)
    warm(leaf, "defaults after")
    rebound_m.__code__ = other_code.__code__
    warm(leaf, "code after")

    # __class__ assignment moves the receiver to another class.
    leaf.__class__ = Other
    warm(leaf, "class assigned")
    leaf.__class__ = Leaf
    warm(leaf, "class restored")

    # A class with no instance dictionary.
    slotted = Slotted()
    warm(slotted, "slotted")
    slotted.v = 9
    warm(slotted, "slotted field")

    # A polymorphic site alternates receivers each call.
    mixed = [Leaf(), Other(), Slotted(), Leaf()]
    print("mixed", [run(obj, 2) for obj in mixed * 3])

    # A non-function attribute on the class.
    Base.m = staticmethod(lambda x, bump=2: ("static", x + bump))
    warm(Leaf(), "staticmethod")
    Base.m = classmethod(lambda cls, x, bump=2: (cls.__name__, x + bump))
    warm(Leaf(), "classmethod")

    # Too few and too many arguments still raise TypeError at the site.
    Base.m = rebound_m
    for args in [(), (1, 2, 3)]:
        try:
            Leaf().m(*args)
        except TypeError as error:
            print("TypeError", type(error).__name__)

    # Deep recursion through a direct site call still raises RecursionError.
    deep = Deep()
    print("deep", deep.down(50))
    try:
        deep.down(100000)
    except RecursionError:
        print("RecursionError")


main()
