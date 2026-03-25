# Parity test: classes and OOP
# All output via print() for diff comparison

print("=== Basic class ===")
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __repr__(self):
        return f"Point({self.x}, {self.y})"
    def __str__(self):
        return f"({self.x}, {self.y})"
    def distance(self):
        return (self.x ** 2 + self.y ** 2) ** 0.5

p = Point(3, 4)
print(p)
print(repr(p))
print(p.x, p.y)
print(p.distance())

print("=== Class variables vs instance variables ===")
class Counter:
    count = 0
    def __init__(self):
        Counter.count += 1
        self.id = Counter.count

a = Counter()
b = Counter()
c = Counter()
print(a.id, b.id, c.id)
print(Counter.count)

print("=== Inheritance ===")
class Animal:
    def __init__(self, name):
        self.name = name
    def speak(self):
        return "..."

class Dog(Animal):
    def speak(self):
        return "Woof!"

class Cat(Animal):
    def speak(self):
        return "Meow!"

animals = [Dog("Rex"), Cat("Whiskers"), Animal("Thing")]
for a in animals:
    print(f"{a.name}: {a.speak()}")

print("=== super() ===")
class Base:
    def __init__(self, x):
        self.x = x
    def method(self):
        return f"Base.method(x={self.x})"

class Derived(Base):
    def __init__(self, x, y):
        super().__init__(x)
        self.y = y
    def method(self):
        return f"Derived.method(x={self.x}, y={self.y})"

d = Derived(1, 2)
print(d.method())
print(d.x, d.y)

print("=== Multiple inheritance ===")
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

print(D().who())
print([cls.__name__ for cls in D.__mro__])

print("=== classmethod and staticmethod ===")
class MyClass:
    class_var = 42

    @classmethod
    def get_class_var(cls):
        return cls.class_var

    @staticmethod
    def static_add(a, b):
        return a + b

print(MyClass.get_class_var())
print(MyClass.static_add(3, 4))
obj = MyClass()
print(obj.get_class_var())
print(obj.static_add(5, 6))

print("=== property ===")
class Temperature:
    def __init__(self, celsius):
        self._celsius = celsius

    @property
    def celsius(self):
        return self._celsius

    @celsius.setter
    def celsius(self, value):
        self._celsius = value

    @property
    def fahrenheit(self):
        return self._celsius * 9 / 5 + 32

t = Temperature(100)
print(t.celsius)
print(t.fahrenheit)
t.celsius = 0
print(t.celsius)
print(t.fahrenheit)

print("=== __eq__, __lt__, __hash__ ===")
class Value:
    def __init__(self, v):
        self.v = v
    def __eq__(self, other):
        if isinstance(other, Value):
            return self.v == other.v
        return NotImplemented
    def __lt__(self, other):
        if isinstance(other, Value):
            return self.v < other.v
        return NotImplemented
    def __hash__(self):
        return hash(self.v)
    def __repr__(self):
        return f"Value({self.v})"

a = Value(1)
b = Value(2)
c = Value(1)
print(a == c)
print(a == b)
print(a < b)
print(b < a)
print(hash(a) == hash(c))

print("=== __len__, __getitem__, __contains__ ===")
class MyList:
    def __init__(self, data):
        self._data = list(data)
    def __len__(self):
        return len(self._data)
    def __getitem__(self, idx):
        return self._data[idx]
    def __contains__(self, item):
        return item in self._data
    def __repr__(self):
        return f"MyList({self._data})"

ml = MyList([10, 20, 30])
print(len(ml))
print(ml[0])
print(ml[-1])
print(20 in ml)
print(99 in ml)

print("=== __iter__ ===")
class Range3:
    def __init__(self, n):
        self.n = n
    def __iter__(self):
        self._i = 0
        return self
    def __next__(self):
        if self._i >= self.n:
            raise StopIteration
        val = self._i
        self._i += 1
        return val

print(list(Range3(5)))

print("=== __add__, __mul__ ===")
class Vec:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __add__(self, other):
        return Vec(self.x + other.x, self.y + other.y)
    def __mul__(self, scalar):
        return Vec(self.x * scalar, self.y * scalar)
    def __repr__(self):
        return f"Vec({self.x}, {self.y})"

v1 = Vec(1, 2)
v2 = Vec(3, 4)
print(v1 + v2)
print(v1 * 3)

print("=== __call__ ===")
class Multiplier:
    def __init__(self, factor):
        self.factor = factor
    def __call__(self, x):
        return x * self.factor

double = Multiplier(2)
triple = Multiplier(3)
print(double(5))
print(triple(5))
print(callable(double))

print("=== isinstance / issubclass ===")
print(isinstance(42, int))
print(isinstance("hi", str))
print(isinstance(True, int))
print(isinstance(True, bool))
print(isinstance([], list))
print(isinstance({}, dict))
print(isinstance(42, (int, str)))
print(isinstance("hi", (int, str)))
print(isinstance(3.14, (int, str)))

print(issubclass(bool, int))
print(issubclass(int, object))
print(issubclass(str, object))

print("=== type() ===")
print(type(42).__name__)
print(type("hi").__name__)
print(type(3.14).__name__)
print(type(True).__name__)
print(type(None).__name__)
print(type([]).__name__)
print(type({}).__name__)
print(type(set()).__name__)
print(type((1,)).__name__)

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

print("=== Abstract-like pattern ===")
class Shape:
    def area(self):
        raise NotImplementedError

class Circle(Shape):
    def __init__(self, r):
        self.r = r
    def area(self):
        return 3.14159 * self.r ** 2

class Rect(Shape):
    def __init__(self, w, h):
        self.w = w
        self.h = h
    def area(self):
        return self.w * self.h

shapes = [Circle(5), Rect(3, 4)]
for s in shapes:
    print(f"{type(s).__name__}: area={s.area()}")

print("=== __bool__ and __len__ ===")
class Falsy:
    def __bool__(self):
        return False

class TruthyLen:
    def __len__(self):
        return 5

class FalsyLen:
    def __len__(self):
        return 0

print(bool(Falsy()))
print(bool(TruthyLen()))
print(bool(FalsyLen()))

print("=== Dataclass-like ===")
class Record:
    def __init__(self, **kwargs):
        for k, v in kwargs.items():
            setattr(self, k, v)
    def __repr__(self):
        attrs = ", ".join(f"{k}={v!r}" for k, v in sorted(self.__dict__.items()))
        return f"Record({attrs})"

r = Record(name="Alice", age=30)
print(r)
print(r.name)
print(r.age)
