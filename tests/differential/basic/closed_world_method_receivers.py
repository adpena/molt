"""Purpose: a method call binds its receiver to the method's first parameter.

A class whose instances only call methods stays provable, so each call below
must still observe every later writer: a class attribute store through
``self.__class__``, through ``type(obj)``, through a leaked instance, through
a base class, an instance attribute that shadows a method, and ``super()``
chains including a diamond.
"""


class Base:
    def compute(self, x):
        return x

    def describe(self):
        return "base"


class Mid(Base):
    def compute(self, x):
        return super().compute(x) + 1


class Leaf(Mid):
    def compute(self, x):
        return super().compute(x) * 2


def hierarchy():
    obj = Leaf()
    total = 0
    for i in range(5):
        total += obj.compute(i)
    print("hierarchy", total, obj.describe())
    Base.describe = lambda self: "patched"
    print("hierarchy", obj.describe())
    Mid.compute = lambda self, x: 1000 + x
    print("hierarchy", obj.compute(1))


class Counter:
    def __init__(self):
        self.count = 0

    def bump(self, k):
        self.count += k
        return self.count

    def rewrite(self):
        self.__class__.bump = lambda self, k: -k


def self_class_writer():
    c = Counter()
    print("writer", c.bump(2), c.bump(3))
    c.rewrite()
    print("writer", c.bump(4), c.count)


class Registry:
    items = []

    def register(self):
        Registry.items.append(self)
        return len(Registry.items)

    def name(self):
        return "registered"


def leaked_receiver():
    r = Registry()
    print("leak", r.register(), r.name())
    leaked = Registry.items[0]
    type(leaked).name = lambda self: "replaced"
    print("leak", r.name(), leaked is r)


class Shadow:
    def value(self):
        return "method"


def instance_shadow():
    s = Shadow()
    print("shadow", s.value())
    s.value = lambda: "instance"
    print("shadow", s.value())
    del s.value
    print("shadow", s.value())


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
    print("diamond", D().who(), B().who(), C().who())
    A.who = lambda self: ["patched-A"]
    print("diamond", D().who())


class Holder:
    def get(self):
        return self

    def tag(self):
        return "holder"


def returned_self():
    h = Holder()
    again = h.get()
    again.__class__.tag = lambda self: "returned"
    print("returned", h.tag(), again is h)


class Bound:
    def __init__(self, n):
        self.n = n

    def read(self):
        return self.n


def bound_method_value():
    b = Bound(7)
    read = b.read
    print("bound", read())
    Bound.read = lambda self: -self.n
    print("bound", read(), b.read())


class Rebinder:
    def swap(self, other):
        self = other
        return super().__repr__() == object.__repr__(other)


def rebound_first_argument():
    print("rebound", Rebinder().swap(Rebinder()))


class Fallback:
    def __getattr__(self, name):
        return lambda *args: (name, args)


def attribute_hook():
    f = Fallback()
    print("hook", f.missing(1, 2))


class Later:
    def run(self):
        return "first"


def setattr_writer():
    obj = Later()
    print("setattr", obj.run())
    setattr(Later, "run", lambda self: "second")
    print("setattr", obj.run())
    Later.__dict__  # a read; the mapping proxy has no store
    print("setattr", Later().run())


class LazyBase:
    def load(self):
        import colorsys

        return colorsys.rgb_to_hsv(1.0, 0.0, 0.0)


class LazySub(LazyBase):
    def load(self):
        return super().load()


class LazyInherited(LazyBase):
    pass


def super_reaches_base_import():
    print("lazy", LazySub().load(), LazyInherited().load())


class Tally:
    def __init__(self):
        self.n = 0

    def add(self, k):
        self.n += k
        return self.n


class Root:
    def twice(self, x):
        return 2 * x


class Child(Root):
    pass


class Victim:
    def act(self):
        return "class"


def shadow_later(obj, step):
    if step == 2:
        obj.act = lambda: "instance"


def loop_callbacks():
    # Each call is a callback, so every receiver's fact is durable inside the
    # loop. Tally and Child stay closed: their methods are direct calls. A
    # callback shadows Victim's method on its instance, which must be seen.
    t = Tally()
    c = Child()
    v = Victim()
    seen = []
    total = 0
    for i in range(4):
        t.add(i)
        total += c.twice(i)
        seen.append(v.act())
        shadow_later(v, i)
    print("loop", t.n, total, seen)


hierarchy()
self_class_writer()
leaked_receiver()
instance_shadow()
diamond()
returned_self()
bound_method_value()
rebound_first_argument()
attribute_hook()
setattr_writer()
super_reaches_base_import()
loop_callbacks()
