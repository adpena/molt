# Parity test: advanced class features
# All output via print() for diff comparison

print("=== Multiple inheritance MRO ===")
class A:
    def who(self):
        return "A"

class B(A):
    def who(self):
        return "B"

class C(A):
    def who(self):
        return "C"

class D(B, C):
    pass

class E(C, B):
    pass

print(D().who())
print(E().who())
print([c.__name__ for c in D.__mro__])
print([c.__name__ for c in E.__mro__])

print("=== super() chains ===")
class Base:
    def __init__(self):
        self.log = []
        self.log.append("Base")

class Left(Base):
    def __init__(self):
        super().__init__()
        self.log.append("Left")

class Right(Base):
    def __init__(self):
        super().__init__()
        self.log.append("Right")

class Bottom(Left, Right):
    def __init__(self):
        super().__init__()
        self.log.append("Bottom")

b = Bottom()
print(b.log)

print("=== super() with methods ===")
class M1:
    def greet(self):
        return "M1"

class M2(M1):
    def greet(self):
        return "M2+" + super().greet()

class M3(M1):
    def greet(self):
        return "M3+" + super().greet()

class M4(M2, M3):
    def greet(self):
        return "M4+" + super().greet()

print(M4().greet())

print("=== __slots__ ===")
class Slotted:
    __slots__ = ('x', 'y')
    def __init__(self, x, y):
        self.x = x
        self.y = y

s = Slotted(1, 2)
print(s.x, s.y)
s.x = 10
print(s.x)

try:
    s.z = 3
except AttributeError:
    print("AttributeError: cannot add z to slotted")

print(hasattr(s, '__dict__'))

print("=== __slots__ with inheritance ===")
class SlottedBase:
    __slots__ = ('a',)

class SlottedChild(SlottedBase):
    __slots__ = ('b',)

sc = SlottedChild()
sc.a = 1
sc.b = 2
print(sc.a, sc.b)

print("=== __init_subclass__ ===")
class Plugin:
    registry = []
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        Plugin.registry.append(cls.__name__)

class PluginA(Plugin):
    pass

class PluginB(Plugin):
    pass

class PluginC(PluginB):
    pass

print(Plugin.registry)

print("=== __init_subclass__ with kwargs ===")
class Validator:
    validators = {}
    def __init_subclass__(cls, type_name=None, **kwargs):
        super().__init_subclass__(**kwargs)
        if type_name:
            Validator.validators[type_name] = cls

class IntValidator(Validator, type_name="int"):
    pass

class StrValidator(Validator, type_name="str"):
    pass

print(sorted(Validator.validators.keys()))

print("=== Descriptors ===")
class Positive:
    def __set_name__(self, owner, name):
        self.name = name
        self.storage = f"_positive_{name}"

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

p = Product("Widget", 10, 5)
print(p.name, p.price, p.quantity)
p.price = 20
print(p.price)

try:
    p.quantity = -1
except ValueError as e:
    print(f"caught: {e}")

print("=== __class_getitem__ ===")
class TypedBox:
    def __class_getitem__(cls, item):
        return f"TypedBox[{item.__name__}]"

print(TypedBox[int])
print(TypedBox[str])

print("=== Abstract base class pattern ===")
class AbstractShape:
    def area(self):
        raise NotImplementedError("subclass must implement area()")

    def describe(self):
        return f"{type(self).__name__} with area {self.area()}"

class Square(AbstractShape):
    def __init__(self, side):
        self.side = side
    def area(self):
        return self.side ** 2

class Triangle(AbstractShape):
    def __init__(self, base, height):
        self.base = base
        self.height = height
    def area(self):
        return 0.5 * self.base * self.height

print(Square(5).describe())
print(Triangle(6, 4).describe())

try:
    AbstractShape().area()
except NotImplementedError as e:
    print(f"caught: {e}")

print("=== __new__ ===")
class Singleton:
    _instance = None
    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

a = Singleton()
b = Singleton()
print(a is b)

print("=== __del__ tracking ===")
deleted = []
class Tracked:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        deleted.append(self.name)

t = Tracked("temp")
del t
print("temp" in deleted)

print("=== __getattr__ / __setattr__ ===")
class DynamicAttrs:
    def __init__(self):
        self._data = {}

    def __getattr__(self, name):
        if name.startswith('_'):
            raise AttributeError(name)
        return self._data.get(name, f"default_{name}")

    def __setattr__(self, name, value):
        if name.startswith('_'):
            super().__setattr__(name, value)
        else:
            self._data[name] = value

d = DynamicAttrs()
print(d.foo)
d.foo = "bar"
print(d.foo)
print(d.missing)

print("=== Mixin pattern ===")
class JSONMixin:
    def to_json_str(self):
        import json
        return json.dumps(self.__dict__, sort_keys=True)

class Printable:
    def display(self):
        return f"{type(self).__name__}({self.__dict__})"

class Entity(JSONMixin, Printable):
    def __init__(self, id, name):
        self.id = id
        self.name = name

e = Entity(1, "test")
print(e.to_json_str())
print(type(e.display()).__name__)

print("=== __contains__ ===")
class EvenNumbers:
    def __contains__(self, item):
        return isinstance(item, int) and item % 2 == 0

evens = EvenNumbers()
print(2 in evens)
print(3 in evens)
print(0 in evens)
print("a" in evens)
