"""Purpose: CPython parity for the fused method / super dispatch fast path.

Exercises the `call_method_ic` / `call_super_method_ic` lowering and their
per-site inline caches.  Every case here must stay byte-identical to CPython:
deep hierarchies + super chains, super() in __init__, overrides below the static
receiver type (polymorphic site — the IC must re-resolve per class), instance
attributes shadowing a method (the IC must defer to the slow path), classmethod
/ staticmethod dispatch, __slots__ instances, a custom __getattribute__ (must
defer to the slow path), and an exception raised through a fused method call.

NOTE: diamond-inheritance super() ordering, property getters folded at module
scope, and same-scope `g.method = fn; g.method()` shadowing are tracked
separately as pre-existing frontend lowering gaps (see the dispatch fix baton);
they reproduce identically with method fusion disabled, so they are independent
of this change and intentionally not asserted here.
"""


# --- Deep hierarchy (4 levels) + super chain --------------------------------
class A:
    def compute(self, x: int) -> int:
        return x


class B(A):
    def compute(self, x: int) -> int:
        return super().compute(x) + 1


class C(B):
    def compute(self, x: int) -> int:
        return super().compute(x) * 2


class D(C):
    def compute(self, x: int) -> int:
        return super().compute(x) - 3


def deep() -> None:
    obj = D()
    total = 0
    for i in range(1000):
        total += obj.compute(i)
    print("deep", total)


# --- super() in __init__ ----------------------------------------------------
class Animal:
    def __init__(self, name: str) -> None:
        self.name = name

    def describe(self) -> str:
        return "animal:" + self.name


class Dog(Animal):
    def __init__(self, name: str, breed: str) -> None:
        super().__init__(name)
        self.breed = breed

    def describe(self) -> str:
        return super().describe() + "/" + self.breed


def super_init() -> None:
    d = Dog("Rex", "Lab")
    print("init", d.name, d.breed, d.describe())


# --- Override below the static receiver type (polymorphic IC site) ----------
class Shape:
    def area(self) -> int:
        return 0


class Square(Shape):
    def __init__(self, side: int) -> None:
        self.side = side

    def area(self) -> int:
        return self.side * self.side


def overridden() -> None:
    shapes = [Shape(), Square(3), Shape(), Square(5), Square(2)]
    total = 0
    for s in shapes:
        total += s.area()
    print("override", total)


# --- Instance attribute shadowing a method (via a helper call site) ---------
class Greeter:
    def hello(self) -> str:
        return "method"


def _call_hello(g: Greeter) -> str:
    return g.hello()


def instance_shadow() -> None:
    g = Greeter()
    out = [_call_hello(g), _call_hello(g)]  # warm the IC
    g.hello = lambda: "shadowed"            # instance attr shadows the method
    out.append(_call_hello(g))
    out.append(_call_hello(Greeter()))      # fresh instance: method again
    print("shadow", out)


# --- classmethod / staticmethod dispatch ------------------------------------
class Registry:
    count = 10

    @classmethod
    def cls_value(cls) -> int:
        return cls.count

    @staticmethod
    def stat_value(x: int) -> int:
        return x + 100


def descriptors() -> None:
    r = Registry()
    print("classmethod", r.cls_value())
    print("staticmethod", r.stat_value(5))


# --- __slots__ class (no instance __dict__) ---------------------------------
class Slotted:
    __slots__ = ("v",)

    def __init__(self, v: int) -> None:
        self.v = v

    def doubled(self) -> int:
        return self.v * 2


def slotted() -> None:
    total = 0
    for i in range(100):
        total += Slotted(i).doubled()
    print("slots", total)


# --- Custom __getattribute__ must defer to the slow path --------------------
class Watched:
    def __getattribute__(self, name: str):
        if name == "ping":
            return lambda: "intercepted"
        return object.__getattribute__(self, name)

    def ping(self) -> str:
        return "real"


def custom_getattribute() -> None:
    w = Watched()
    print("getattribute", _call_ping(w), _call_ping(w))


def _call_ping(w: Watched) -> str:
    return w.ping()


# --- Exception raised through a fused method call ---------------------------
class Boom:
    def explode(self, n: int) -> int:
        if n > 2:
            raise ValueError("kaboom " + str(n))
        return n


def _call_explode(b: Boom, n: int) -> int:
    return b.explode(n)


def raising() -> None:
    # The exception raised inside the fused `b.explode(n)` call must propagate
    # and be catchable; the IC must not swallow or mis-route it.
    b = Boom()
    ok = 0
    caught = 0
    for i in range(5):
        try:
            ok += _call_explode(b, i)
        except ValueError:
            caught += 1
    print("raise ok", ok, "caught", caught)


def main() -> None:
    deep()
    super_init()
    overridden()
    instance_shadow()
    descriptors()
    slotted()
    custom_getattribute()
    raising()


if __name__ == "__main__":
    main()
