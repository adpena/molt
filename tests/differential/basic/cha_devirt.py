"""Purpose: CHA devirtualization + static super-chain folding soundness.

molt forbids runtime monkeypatching and compiles whole-program AOT, so the
class hierarchy is closed and statically known.  When the receiver's exact
class is statically proven (``obj = Leaf()`` then ``obj.compute(i)``) the
method call is devirtualized to a direct call and the active inliner collapses
the whole ``super()`` chain to arithmetic.

This test pins the soundness boundary of that fold, with special attention to
methods that close over the implicit ``__class__`` super cell (any method using
zero-arg ``super()``):

  * A method whose closure is exactly ``__class__`` IS inline-eligible, but only
    when every ``super()`` in its body folds statically — the inlined body is
    spliced into a scope with no ``__class__`` cell, so an un-foldable ``super()``
    would otherwise raise ``RuntimeError: super(): __class__ cell not found``.
  * A diamond reached through an exact local must NOT be statically super-folded
    (its cooperative C3 successor differs from the lexical one); the call routes
    through the cell-threaded runtime super path and stays correct.
  * A bare ``__class__`` *value* load inside an exact-local method must keep
    working (the fold declines to inline it; dispatch threads the cell).

Output must be byte-identical to CPython 3.14.
"""


# --- Linear chain reached through an exact local: full super fold -----------
class Base:
    def compute(self, x: int) -> int:
        return x


class Mid(Base):
    def compute(self, x: int) -> int:
        return super().compute(x) + 1


class Leaf(Mid):
    def compute(self, x: int) -> int:
        return super().compute(x) * 2


def linear_chain() -> None:
    obj = Leaf()  # exact class proven -> devirt + inline the super chain
    total = 0
    for i in range(1000):
        total += obj.compute(i)
    print("linear", total)


# --- Diamond reached through an exact local: NO static super fold -----------
class DBase:
    def who(self) -> str:
        return "DBase"


class DLeft(DBase):
    def who(self) -> str:
        return "DLeft->" + super().who()


class DRight(DBase):
    def who(self) -> str:
        return "DRight->" + super().who()


class DFinal(DLeft, DRight):
    def who(self) -> str:
        return "DFinal->" + super().who()


def diamond_exact() -> None:
    # DFinal's C3 MRO is [DFinal, DLeft, DRight, DBase]; DLeft.who's super()
    # must reach DRight on a DFinal instance.  Reached through an exact local
    # so the devirt site sees DFinal: the fold must decline (cooperative C3
    # successor differs) and route through the runtime super path.
    f = DFinal()
    print("diamond", f.who())
    left = DLeft()  # a DLeft instance: its super() reaches DBase directly
    print("diamond_left", left.who())


# --- Override below a polymorphic site: devirt must REFUSE ------------------
class Shape:
    def area(self) -> int:
        return 1


class Circle(Shape):
    def area(self) -> int:
        return 7


def polymorphic() -> None:
    # `s` is not an exact local (loop variable over a heterogeneous list), so
    # the devirt must refuse and dispatch dynamically per element.
    shapes = [Shape(), Circle(), Shape(), Circle(), Circle()]
    total = 0
    for s in shapes:
        total += s.area()
    print("poly", total)


# --- Method that loads bare __class__ as a value ----------------------------
class Named:
    def label(self) -> str:
        # Bare __class__ value load: the fold declines to inline this (the
        # inlined body would have no cell); dispatch threads the cell.
        return __class__.__name__ + ":" + str(self.compute_one())

    def compute_one(self) -> int:
        return 1


class NamedSub(Named):
    def compute_one(self) -> int:
        return super().compute_one() + 41


def bare_classcell() -> None:
    n = Named()
    print("named", n.label())
    s = NamedSub()
    print("named_sub", s.label())


# --- super() inside __init__ through an exact local -------------------------
class Vehicle:
    def __init__(self, wheels: int) -> None:
        self.wheels = wheels

    def info(self) -> str:
        return "wheels:" + str(self.wheels)


class Car(Vehicle):
    def __init__(self, doors: int) -> None:
        super().__init__(4)
        self.doors = doors

    def info(self) -> str:
        return super().info() + "/doors:" + str(self.doors)


def init_chain() -> None:
    c = Car(2)  # exact class -> __init__ super chain + info super chain fold
    print("car", c.info())


def main() -> None:
    linear_chain()
    diamond_exact()
    polymorphic()
    bare_classcell()
    init_chain()


if __name__ == "__main__":
    main()
