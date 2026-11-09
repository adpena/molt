"""Purpose: a data descriptor added after definition shadows an inferred field.

The class statements publish inferred field slots. Code reaches each class
through an instance (``type(obj)``, ``__bases__``, ``__mro__``) and installs a
property on the field's name. CPython's data-descriptor precedence then wins
over the instance's own value: for the class that owns the field, for methods
of a subclass whose base gains the descriptor, and for later constructions,
whose field store now meets a property without a setter.
"""


class Leaf:
    def __init__(self, value):
        self.y = value

    def read(self):
        return self.y


class Base:
    def __init__(self, value):
        self.x = value

    def read(self):
        return self.x


class Child(Base):
    def __init__(self, value):
        self.x = value

    def read_child(self):
        return self.x


class DeepBase:
    def __init__(self, value):
        self.z = value


class Middle(DeepBase):
    pass


class Deep(Middle):
    def __init__(self, value):
        self.z = value

    def read_deep(self):
        return self.z


def reach(obj):
    return type(obj)


leaf = Leaf(1)
print("leaf before", leaf.read(), leaf.y)
setattr(reach(leaf), "y", property(lambda self: 99))
print("leaf after", leaf.read(), leaf.y)

child = Child(2)
print("child before", child.read(), child.read_child(), child.x)
setattr(reach(child).__bases__[0], "x", property(lambda self: 77))
print("child after", child.read(), child.read_child(), child.x)
try:
    Child(3)
except AttributeError as error:
    print("child construct", type(error).__name__)

deep = Deep(4)
print("deep before", deep.read_deep(), deep.z)
setattr(reach(deep).__mro__[2], "z", property(lambda self: 55))
print("deep after", deep.read_deep(), deep.z)
