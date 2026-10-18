# Parity test: typing module features
# All output via print() for diff comparison

from typing import (
    TypeVar,
    Generic,
    Protocol,
    Union,
    Optional,
    List,
    Dict,
    Callable,
    Tuple,
    Any,
)

print("=== Basic type hints (runtime access) ===")


def greet(name: str) -> str:
    return f"Hello, {name}"


print(greet("world"))
print(greet.__annotations__)

print("=== TypeVar ===")
T = TypeVar("T")
U = TypeVar("U")


def identity(x: T) -> T:
    return x


print(identity(42))
print(identity("hello"))
print(identity([1, 2, 3]))

print("=== Generic class ===")


class Box(Generic[T]):
    def __init__(self, value: T):
        self.value = value

    def get(self) -> T:
        return self.value

    def __repr__(self):
        return f"Box({self.value!r})"


b1 = Box(42)
b2 = Box("hello")
print(b1)
print(b2)
print(b1.get())
print(b2.get())

print("=== Generic with multiple type params ===")


class Pair(Generic[T, U]):
    def __init__(self, first: T, second: U):
        self.first = first
        self.second = second

    def __repr__(self):
        return f"Pair({self.first!r}, {self.second!r})"


p = Pair(1, "one")
print(p)
print(p.first, p.second)

print("=== Optional ===")


def find_index(lst: List[int], val: int) -> Optional[int]:
    for i, x in enumerate(lst):
        if x == val:
            return i
    return None


print(find_index([10, 20, 30], 20))
print(find_index([10, 20, 30], 99))

print("=== Union ===")


def process(val: Union[int, str]) -> str:
    if isinstance(val, int):
        return f"int:{val}"
    return f"str:{val}"


print(process(42))
print(process("hello"))

print("=== isinstance with basic types ===")
print(isinstance(42, int))
print(isinstance("hi", str))
print(isinstance([1], list))
print(isinstance({"a": 1}, dict))
print(isinstance((1, 2), tuple))
print(isinstance(True, bool))
print(isinstance(True, int))
print(isinstance(3.14, float))

print("=== Callable annotations ===")


def apply(func: Callable[[int, int], int], a: int, b: int) -> int:
    return func(a, b)


print(apply(lambda x, y: x + y, 3, 4))
print(apply(lambda x, y: x * y, 3, 4))

print("=== Type checking with type() ===")
print(type(42) is int)
print(type("hi") is str)
print(type([]) is list)
print(type({}) is dict)
print(type(()) is tuple)
print(type(True) is bool)
print(type(None) is type(None))

print("=== Protocol (structural typing) ===")


class Drawable(Protocol):
    def draw(self) -> str: ...


class Circle:
    def draw(self) -> str:
        return "Circle.draw()"


class Square:
    def draw(self) -> str:
        return "Square.draw()"


class NotDrawable:
    pass


def render(shape: Drawable) -> str:
    return shape.draw()


print(render(Circle()))
print(render(Square()))

print("=== runtime_checkable Protocol ===")
from typing import runtime_checkable


@runtime_checkable
class Sized(Protocol):
    def __len__(self) -> int: ...


print(isinstance([1, 2], Sized))
print(isinstance("abc", Sized))
print(isinstance(42, Sized))
print(isinstance({}, Sized))

print("=== __class_getitem__ ===")
print(List[int])
print(Dict[str, int])
print(Tuple[int, str])
print(Optional[int])

print("=== Type alias patterns ===")
Vector = List[float]
Matrix = List[List[float]]

v: Vector = [1.0, 2.0, 3.0]
m: Matrix = [[1.0, 0.0], [0.0, 1.0]]
print(v)
print(m)

print("=== Any type ===")


def accepts_any(x: Any) -> str:
    return str(x)


print(accepts_any(42))
print(accepts_any("hello"))
print(accepts_any([1, 2]))
print(accepts_any(None))

print("=== TypeVar with bound ===")


class Comparable:
    def __init__(self, val):
        self.val = val

    def __lt__(self, other):
        return self.val < other.val

    def __repr__(self):
        return f"Comparable({self.val})"


CT = TypeVar("CT", bound=Comparable)


def min_val(a: CT, b: CT) -> CT:
    return a if a < b else b


print(min_val(Comparable(3), Comparable(1)))
print(min_val(Comparable(5), Comparable(9)))
