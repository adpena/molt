"""Static zero-argument super() (design 78, 5.10 step 4).

A super() call on an exact receiver whose selection the compiler can decide
enters the selected method directly; every case CPython decides at run time
keeps the dynamic path and the same output.
"""


class Base:
    def compute(self, x):
        return x + 1

    @classmethod
    def make(cls):
        return cls.__name__

    @staticmethod
    def helper(x):
        return x * 10


class Mid(Base):
    def compute(self, x):
        return super().compute(x) * 2

    @classmethod
    def make(cls):
        return "Mid:" + super().make()

    def help(self, x):
        return super().helper(x)


class Leaf(Mid):
    def compute(self, x):
        return super().compute(x) - 3

    def plain(self):
        return super().make()


def static_chain():
    leaf = Leaf()
    total = 0
    for i in range(200):
        total += leaf.compute(i)
    print("chain", total, leaf.plain(), Leaf.make(), leaf.help(4))


class Rebinder(Base):
    def compute(self, x):
        return "rebinder " + str(super().compute(x))

    def rebind(self):
        nonlocal __class__
        __class__ = Base


def rebound_cell():
    obj = Rebinder()
    print("before", obj.compute(1))
    obj.rebind()
    try:
        print("after", obj.compute(1))
    except AttributeError as exc:
        print("after", type(exc).__name__)


class Stack(list):
    def append(self, item):
        super().append(item * 2)


def builtin_base():
    s = Stack()
    for i in range(3):
        s.append(i)
    print("builtin", list(s))


class Meta(type):
    def __new__(mcls, name, bases, ns):
        return super().__new__(mcls, name, bases, ns)


class Metered(Base, metaclass=Meta):
    def compute(self, x):
        return super().compute(x) + 100


def custom_metaclass():
    print("metaclass", Metered().compute(1))


class Shadow(Base):
    compute = None


class AfterShadow(Shadow):
    def compute(self, x):
        try:
            return super().compute(x)
        except TypeError as exc:
            return type(exc).__name__


def shadowed():
    print("shadow", AfterShadow().compute(5))


class Arity(Base):
    def compute(self, x, y=7):
        return super().compute(x) + y


class ArityLeaf(Arity):
    def compute(self, x):
        return super().compute(x) + super().compute(x, 1)


def defaults():
    print("defaults", ArityLeaf().compute(2))


static_chain()
rebound_cell()
builtin_base()
custom_metaclass()
shadowed()
defaults()
