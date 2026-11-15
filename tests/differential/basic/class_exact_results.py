"""Purpose: a constructor's result stays exactly its class across callbacks
when no code in the program can assign `__class__` (HF-847: the closed-world
writer fact makes its field accesses machine code). This program holds no
`__class__` store and no computed-name setattr, so the fact holds. Each block
separates the correct lowering from a plausible defect: a field read that
misses an instance dictionary a callback created, a deleted field read as a
value, a method resolved on the class while a callback stored an instance
attribute that shadows it, and a stale fact on a rebound local."""


class Point:
    x: int
    y: int

    def __init__(self, x, y):
        self.x = x
        self.y = y

    def norm(self):
        return abs(self.x) + abs(self.y)


class Noisy:
    """A field value whose finalizer runs Python code during a store."""

    log = []

    def __init__(self, tag, target=None):
        self.tag = tag
        self.target = target

    def __del__(self):
        Noisy.log.append(self.tag)
        if self.target is not None:
            # A callback that gives the target an instance dictionary and
            # shadows a method with an instance attribute.
            self.target.extra = self.tag
            self.target.norm = lambda: "shadowed"


def callbacks_between_accesses():
    p = Point(1, 2)
    p.x = Noisy("a", p)
    p.x = 5  # releases Noisy("a"): its __del__ writes p.extra and p.norm
    print(p.x, p.y, p.extra, p.norm(), sorted(vars(p)))
    p.y = p.x + 1
    print(p.x, p.y, Noisy.log)


def dictionary_then_read():
    p = Point(3, 4)
    d = vars(p)
    d["x"] = 30
    print(p.x, p.y)
    p.x = 31
    print(d["x"], p.x)


def deleted_then_read():
    p = Point(5, 6)
    del p.x
    try:
        print(p.x)
    except AttributeError:
        print("AttributeError")
    p.x = 7
    print(p.x, p.norm())


def rebound_local(flag):
    p = Point(1, 1)
    p.x = 2
    if flag:
        p = "not a point"
    try:
        print(p.x)
    except AttributeError:
        print("str has no x")


def loop_of_constructions(n):
    total = 0
    keep = []
    i = 0
    while i < n:
        p = Point(i, i + 1)
        p.x = p.x * 2
        p.y = [p.y]
        if i % 100 == 0:
            keep.append(p)
        total += p.x + p.y[0]
        i += 1
    print(total, [(k.x, k.y) for k in keep[:3]])


class Base:
    def __init__(self, v):
        self.v = v


class Derived(Base):
    def __init__(self, v):
        super().__init__(v)
        self.w = v * 2


def subclass_values():
    items = [Base(1), Derived(2), Base(3)]
    for item in items:
        item.v = item.v + 10
    print([(type(i).__name__, i.v, getattr(i, "w", None)) for i in items])


callbacks_between_accesses()
dictionary_then_read()
deleted_then_read()
rebound_local(False)
rebound_local(True)
loop_of_constructions(1000)
subclass_values()
