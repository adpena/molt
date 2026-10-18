"""Purpose: an instance store reaches a class data descriptor however it is bound.

The first statement is a call that may run arbitrary Python, so no later class
read has a proved class identity. Every store below must still call
``__set__``: through an unresolved descriptor class, an inherited ``__set__``,
a factory call, a shared instance, a decorator that returns a property, a
``__delete__``-only descriptor, a base-class attribute and an augmented store.
"""

print("start")


def check(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, "raised", type(exc).__name__, exc)


class Positive:
    def __set_name__(self, owner, name):
        self.name = name
        self.storage = "_positive_" + name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return getattr(obj, self.storage, 0)

    def __set__(self, obj, value):
        if value < 0:
            raise ValueError(f"{self.name} must be positive, got {value}")
        setattr(obj, self.storage, value)


class Product:
    price = Positive()
    quantity = Positive()

    def __init__(self, name, price, quantity):
        self.name = name
        self.price = price
        self.quantity = quantity


product = Product("Widget", 10, 5)
print("unresolved-class", product.name, product.price, product.quantity)
product.price = 20
print("rebound", product.price, sorted(vars(product)))
try:
    product.quantity = -1
except ValueError as exc:
    print("validated", exc)


def as_property(getter):
    return property(getter, lambda self, value: setattr(self, "_v", value * 2))


class Decorated:
    @as_property
    def v(self):
        return self._v

    def __init__(self):
        self.v = 3


check("decorated", lambda: Decorated().v)


class BaseDescriptor:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return obj.__dict__.get("_x", -1)

    def __set__(self, obj, value):
        obj.__dict__["_x"] = value * 10


class InheritedDescriptor(BaseDescriptor):
    pass


class Inherits:
    x = InheritedDescriptor()

    def __init__(self):
        self.x = 4


check("inherited-set", lambda: Inherits().x)


def make_descriptor():
    return BaseDescriptor()


class Factory:
    x = make_descriptor()

    def __init__(self):
        self.x = 5


check("factory", lambda: Factory().x)

shared = BaseDescriptor()


class Shared:
    x = shared

    def __init__(self):
        self.x = 6


check("shared", lambda: Shared().x)


class DeleteOnly:
    def __get__(self, obj, objtype=None):
        return "from-get"

    def __delete__(self, obj):
        pass


class UsesDeleteOnly:
    y = DeleteOnly()

    def __init__(self):
        self.y = 7


check("delete-only", lambda: UsesDeleteOnly().y)


class Base:
    x = InheritedDescriptor()


class Child(Base):
    def __init__(self):
        self.x = 8


check("base-class", lambda: Child().x)


class Augmented:
    x = InheritedDescriptor()

    def __init__(self):
        self.x = 1
        self.x += 1


check("augmented", lambda: Augmented().x)


class TupleBound:
    x, y = BaseDescriptor(), 0

    def __init__(self):
        self.x = 9
        self.y = 1


check("tuple-target", lambda: (TupleBound().x, TupleBound().y))


class Plain:
    count = 0
    limit = -1

    def __init__(self):
        self.count = 9
        self.limit = 2


check("plain", lambda: (Plain().count, Plain.count, Plain().limit, Plain.limit))
