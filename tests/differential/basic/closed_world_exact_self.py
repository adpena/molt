"""Purpose: a method's receiver is exactly its class only when every entry proves it.

Each program below gives a plain method a receiver its class statement did not
build: an unbound call through the class, a subclass instance, a foreign object
with the same field, a rebound first parameter, a method value extracted from
the class, and a receiver read back from a container. Field reads and stores in
the method must still see the real receiver.
"""


class Counter:
    def __init__(self):
        self.count = 0

    def bump(self, k):
        self.count += k
        return self.count


def exact_receiver():
    c = Counter()
    total = 0
    for i in range(10):
        total += c.bump(i & 3)
    print("exact", total, c.count)


class Account:
    def __init__(self, balance):
        self.balance = balance

    def deposit(self, amount):
        self.balance = self.balance + amount
        return self.balance


class Ledger:
    def __init__(self):
        self.extra = "ledger"
        self.balance = 100


def unbound_call_with_foreign_receiver():
    a = Account(5)
    print("unbound", a.deposit(1))
    ledger = Ledger()
    print("unbound", Account.deposit(ledger, 7), ledger.extra)


class Shape:
    def __init__(self):
        self.sides = 0

    def grow(self):
        self.sides += 1
        return self.sides


class Square(Shape):
    def __init__(self):
        self.label = "square"
        self.sides = 4


def subclass_receiver():
    s = Shape()
    q = Square()
    print("subclass", s.grow(), q.grow(), q.label)


class Point:
    def __init__(self, x):
        self.x = x

    def moved(self, other):
        self = other
        return self.x


class Other:
    def __init__(self):
        self.pad = 1
        self.x = "other"


def rebound_first_parameter():
    p = Point(3)
    print("rebound", p.moved(Other()), p.moved(Point(9)))


class Box:
    def __init__(self, item):
        self.item = item

    def get(self):
        return self.item


class Crate:
    def __init__(self):
        self.size = 2
        self.item = "crate"


def extracted_method_value():
    b = Box("box")
    getter = Box.get
    print("extracted", getter(b), getter(Crate()))
    bound = b.get
    print("extracted", bound())


class Cell:
    def __init__(self, value):
        self.value = value

    def read(self):
        return self.value


def receiver_from_container():
    cells = [Cell(i) for i in range(3)]
    total = 0
    for cell in cells:
        total += cell.read()
    print("container", total)


class Missing:
    def __init__(self, flag):
        if flag:
            self.value = "set"

    def read(self):
        return self.value


def missing_field_raises():
    print("missing", Missing(True).read())
    try:
        Missing(False).read()
    except AttributeError as exc:
        print("missing", type(exc).__name__, "value" in str(exc))


exact_receiver()
unbound_call_with_foreign_receiver()
subclass_receiver()
rebound_first_parameter()
extracted_method_value()
receiver_from_container()
missing_field_raises()
