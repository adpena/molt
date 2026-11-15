"""Purpose: `C(args)` keeps CPython's construction semantics when the compiler
allocates the instance inline and enters `__init__` directly (HF-847,
CALL_ALLOC_AND_ENTER_INIT). Each block separates the correct lowering from a
plausible defect: a wrong reference count on the new object, a skipped
`__init__`, a subclass that overrides `__init__` or `__new__`, a class name
rebound after the first call, and the errors the runtime class call owns."""

import weakref
from dataclasses import dataclass


class Point:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


def struct_loop(n):
    # Stores from outside the class (the bench_struct shape) and live
    # instances kept across many allocations: an over-released object would
    # be reused and its fields overwritten.
    keep = []
    total = 0
    i = 0
    while i < n:
        p = Point(i, -i)
        p.x = i * 2
        p.y = i + 1
        if i % 97 == 0:
            keep.append(p)
        total += p.x - p.y
        i += 1
    churn = [Point(k, k) for k in range(2000)]
    print(total, len(keep), len(churn))
    print([(q.x, q.y) for q in keep[:4]], keep[-1].x, keep[-1].y)
    print(keep[0] is not keep[1], type(keep[0]).__name__)


struct_loop(5000)


class Node:
    # Not a field-assignment body: a display, a branch and a method call,
    # so construction enters the compiled `__init__`.
    def __init__(self, value, scale):
        self.value = value * scale
        self.items = [value, scale]
        if value < 0:
            self.sign = "neg"
            return
        self.sign = self.classify()

    def classify(self):
        return "zero" if self.value == 0 else "pos"


def build_nodes():
    nodes = [Node(v, 3) for v in (-2, 0, 5)]
    for node in nodes:
        node.value += 1
    print([(n.value, n.items, n.sign) for n in nodes])
    chain = None
    for k in range(5):
        link = Node(k, 1)
        link.items.append(chain)
        chain = link
    depth = 0
    while chain is not None:
        depth += 1
        chain = chain.items[2]
    print("depth", depth)


build_nodes()

order = []


def tag(label, value):
    order.append(label)
    return value


class Logged:
    def __init__(self, a, b):
        order.append("init")
        self.total = a + b


def argument_order():
    made = Logged(tag("a", 1), tag("b", 2))
    print(order, made.total)


argument_order()

released = []


class Fails:
    def __init__(self, value):
        released.append(weakref.ref(self))
        self.value = value
        if value > 1:
            raise ValueError("bad value %d" % value)


def failing_init():
    ok = Fails(1)
    try:
        Fails(7)
    except ValueError as exc:
        print("caught", exc)
    print("half-built released", released[1]() is None, "kept", released[0]() is ok)


failing_init()


class ReturnsValue:
    def __init__(self):
        self.ready = True
        return 3


def bad_return():
    try:
        ReturnsValue()
    except TypeError as exc:
        print("TypeError", exc)


bad_return()


def arity_errors():
    try:
        Node(1)
    except TypeError as exc:
        print("TypeError", exc)
    try:
        Node(1, 2, 3)
    except TypeError as exc:
        print("TypeError", exc)
    defaulted = Point()
    keyword = Point(y=4)
    print(defaulted.x, defaulted.y, keyword.x, keyword.y)


arity_errors()


class Base:
    def __init__(self, a):
        self.a = a
        self.kind = "base"


class OverridesInit(Base):
    def __init__(self, a):
        self.a = a * 10
        self.kind = "sub"


class OverridesNew(Base):
    def __new__(cls, a):
        obj = object.__new__(cls)
        obj.created = "new"
        return obj


class NewReturnsOther(Base):
    def __new__(cls, a):
        return Base(a + 100)


def hierarchy():
    for obj in (Base(1), OverridesInit(2), OverridesNew(3), NewReturnsOther(4)):
        print(type(obj).__name__, obj.a, obj.kind, getattr(obj, "created", None))


hierarchy()


class Morph:
    def __init__(self, value):
        self.value = value
        self.__class__ = Morphed


class Morphed:
    value = 0

    def describe(self):
        return "morphed %d" % self.value


def class_assignment_in_init():
    m = Morph(5)
    m.value = 6
    print(type(m).__name__, m.describe())


class_assignment_in_init()


class Original:
    def __init__(self, v):
        self.v = v


def make_original(v):
    return Original(v)


first = make_original(1)


class Replacement:
    def __init__(self, v):
        self.v = -v


Original = Replacement
second = make_original(2)
print(type(first).__name__, first.v, type(second).__name__, second.v)


class Registry:
    seen = []

    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        Registry.seen.append(cls.__name__)

    def __init__(self, label):
        self.label = label


class Plugin(Registry):
    def __init__(self, label):
        self.label = "plugin:" + label


class Box:
    def __class_getitem__(cls, item):
        return "Box[%s]" % item.__name__

    def __init__(self, content):
        self.content = content


@dataclass
class Pair:
    left: int
    right: int = 5


class Slotted:
    __slots__ = ("left", "right")

    def __init__(self, left, right):
        self.left = left
        self.right = right


def protocol_classes():
    print(Registry.seen, Registry("r").label, Plugin("p").label)
    print(Box[int], Box(9).content)
    s = Slotted(1, 2)
    s.left = 3
    print(s.left, s.right, hasattr(s, "__dict__"))
    pair = Pair(1)
    pair.right += 1
    print(pair, Pair(2, 3) == Pair(2, 3))


protocol_classes()


class Empty:
    pass


def no_init():
    e = Empty()
    e.note = "set later"
    print(type(e).__name__, e.note, Empty() is not Empty())
    try:
        Empty(1)
    except TypeError as exc:
        print("TypeError", exc)


no_init()
