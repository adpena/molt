# Parity test: dataclasses
# All output via print() for diff comparison

from dataclasses import dataclass, field, asdict, astuple, replace, FrozenInstanceError

print("=== Basic dataclass ===")
@dataclass
class Point:
    x: int
    y: int

p = Point(1, 2)
print(p)
print(p.x, p.y)
print(repr(p))

print("=== Equality ===")
p1 = Point(1, 2)
p2 = Point(1, 2)
p3 = Point(3, 4)
print(p1 == p2)
print(p1 == p3)
print(p1 != p3)

print("=== Default values ===")
@dataclass
class Config:
    name: str
    debug: bool = False
    count: int = 0

c1 = Config("test")
c2 = Config("prod", True, 5)
print(c1)
print(c2)

print("=== field() with default_factory ===")
@dataclass
class Container:
    items: list = field(default_factory=list)
    metadata: dict = field(default_factory=dict)

a = Container()
b = Container()
a.items.append(1)
print(a.items)
print(b.items)  # should be independent
print(a.metadata)

print("=== __post_init__ ===")
@dataclass
class Circle:
    radius: float
    area: float = field(init=False)

    def __post_init__(self):
        self.area = 3.14159 * self.radius ** 2

c = Circle(5.0)
print(c)
print(c.area)

print("=== frozen dataclass ===")
@dataclass(frozen=True)
class FrozenPoint:
    x: int
    y: int

fp = FrozenPoint(1, 2)
print(fp)
try:
    fp.x = 10
except FrozenInstanceError:
    print("FrozenInstanceError raised")

print("=== frozen hashing ===")
fp1 = FrozenPoint(1, 2)
fp2 = FrozenPoint(1, 2)
fp3 = FrozenPoint(3, 4)
print(fp1 == fp2)
print(hash(fp1) == hash(fp2))
print(fp1 == fp3)

s = {fp1, fp2, fp3}
print(len(s))

print("=== Dataclass inheritance ===")
@dataclass
class Base:
    x: int
    y: int

@dataclass
class Child(Base):
    z: int = 0

ch = Child(1, 2, 3)
print(ch)
print(ch.x, ch.y, ch.z)
ch2 = Child(1, 2)
print(ch2)

print("=== asdict ===")
@dataclass
class Person:
    name: str
    age: int

p = Person("Alice", 30)
d = asdict(p)
print(sorted(d.items()))

print("=== astuple ===")
t = astuple(p)
print(t)
print(type(t).__name__)

print("=== replace ===")
p2 = replace(p, age=31)
print(p2)
print(p)  # original unchanged

print("=== Nested dataclass ===")
@dataclass
class Address:
    street: str
    city: str

@dataclass
class Employee:
    name: str
    address: Address

emp = Employee("Bob", Address("123 Main", "Springfield"))
print(emp)
d = asdict(emp)
print(d["name"])
print(d["address"]["city"])

print("=== field repr=False ===")
@dataclass
class Secret:
    name: str
    password: str = field(repr=False)

s = Secret("admin", "hunter2")
print(s)  # password should not appear

print("=== field compare=False ===")
@dataclass
class CacheEntry:
    key: str
    value: int
    hits: int = field(default=0, compare=False)

e1 = CacheEntry("a", 1, hits=10)
e2 = CacheEntry("a", 1, hits=99)
print(e1 == e2)

print("=== Ordering ===")
@dataclass(order=True)
class Priority:
    level: int
    name: str

items = [Priority(3, "low"), Priority(1, "high"), Priority(2, "med")]
print(sorted(items))
print(Priority(1, "a") < Priority(2, "b"))
print(Priority(1, "a") < Priority(1, "b"))
