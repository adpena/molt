"""Purpose: an inherited method reads a subclass instance through the generic path, and a same-shaped class bound to the base's name cannot borrow the base's field offsets (HF-220)."""


class Base:
    def __init__(self):
        self.a = 1

    def get_a(self):
        return self.a

    def set_a(self, value):
        self.a = value


class Child(Base):
    def __init__(self):
        self.z = 5
        super().__init__()


class Other:
    # The same methods as Base give Other Base's layout version.
    def __init__(self):
        self.pad = 7
        self.a = 3

    def get_a(self):
        return self.a

    def set_a(self, value):
        self.a = value


c = Child()
print(c.get_a(), c.z)
c.set_a(9)
print(c.a, c.z, c.get_a())
original = Base
Base = Other
o = Other()
print(original.get_a(o))
original.set_a(o, 11)
print(o.pad, o.a)
print(c.get_a(), original.get_a(original()))
