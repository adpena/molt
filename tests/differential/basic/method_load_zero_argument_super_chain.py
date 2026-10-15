"""Purpose: zero-argument super() method calls through a chain match CPython (HF-216).

Base/Mid/Leaf like bench_class_hierarchy, super() with several arguments,
multiple inheritance, super() in __init__ and in a classmethod, and super()
after the first argument was rebound in the method body.
"""


class Base:
    def compute(self, x):
        return x

    def describe(self, a, b, c):
        return ("base", a, b, c)


class Mid(Base):
    def compute(self, x):
        return super().compute(x) + 1

    def describe(self, a, b, c):
        return ("mid",) + super().describe(a, b, c)


class Leaf(Mid):
    def compute(self, x):
        return super().compute(x) * 2


leaf = Leaf()
total = 0
for index in range(1000):
    total += leaf.compute(index)
print(total)
print(leaf.describe(1, 2, 3))


class Left(Base):
    def compute(self, x):
        return ["left"] + super().compute(x)


class Right(Base):
    def compute(self, x):
        return ["right"] + x


class Diamond(Left, Right):
    def compute(self, x):
        return ["diamond"] + super().compute(x)


print(Diamond().compute([]))


class Named:
    def __init__(self, name):
        self.name = name


class Tagged(Named):
    def __init__(self, name, tag):
        super().__init__(name)
        self.tag = tag


tagged = Tagged("n", "t")
print(tagged.name, tagged.tag)


class Factory:
    @classmethod
    def make(cls, value):
        return (cls.__name__, value)


class SubFactory(Factory):
    @classmethod
    def make(cls, value):
        return ("sub",) + super().make(value)


print(SubFactory.make(9))


class Rebinding(Base):
    def compute(self, x):
        self = Leaf()
        return super().compute(x)


try:
    print(Rebinding().compute(3))
except TypeError as error:
    print("TypeError", error)
