"""Purpose: differential coverage for typing module — Protocol, TypeVar, Generic, Literal, etc."""

from typing import (
    TYPE_CHECKING,
    Annotated,
    ClassVar,
    Generic,
    Literal,
    Protocol,
    TypeVar,
    Union,
    cast,
    get_args,
    get_origin,
    get_type_hints,
    overload,
    runtime_checkable,
)

# ---------- TYPE_CHECKING guard ----------
print("TYPE_CHECKING", TYPE_CHECKING)

# ---------- runtime_checkable Protocol — basic ----------

@runtime_checkable
class Drawable(Protocol):
    def draw(self) -> str: ...


class Circle:
    def draw(self) -> str:
        return "circle"


class Square:
    def draw(self) -> str:
        return "square"


class Rock:
    pass


class FakeDraw:
    draw: int = 42  # attribute, not a method — still satisfies structural check


print("circle isinstance Drawable", isinstance(Circle(), Drawable))
print("square isinstance Drawable", isinstance(Square(), Drawable))
print("rock isinstance Drawable", isinstance(Rock(), Drawable))
# Note: FakeDraw has 'draw' attribute so isinstance returns True (structural)
print("fakedraw isinstance Drawable", isinstance(FakeDraw(), Drawable))

# ---------- Protocol with property and class variable ----------

@runtime_checkable
class Sized(Protocol):
    @property
    def size(self) -> int: ...


class Box:
    @property
    def size(self) -> int:
        return 10


class EmptyBox:
    size = 5  # plain attribute also satisfies property check


print("box isinstance Sized", isinstance(Box(), Sized))
print("emptybox isinstance Sized", isinstance(EmptyBox(), Sized))

# ---------- Protocol inheritance ----------

@runtime_checkable
class Named(Protocol):
    name: str


@runtime_checkable
class NamedDrawable(Named, Drawable, Protocol):
    pass


class Widget:
    name: str = "widget"

    def draw(self) -> str:
        return "widget-draw"


class HalfWidget:
    name: str = "half"
    # missing draw


print("widget isinstance NamedDrawable", isinstance(Widget(), NamedDrawable))
print("halfwidget isinstance NamedDrawable", isinstance(HalfWidget(), NamedDrawable))

# ---------- non-runtime-checkable Protocol raises TypeError ----------

class NonRuntimeProto(Protocol):
    def foo(self) -> int: ...


try:
    isinstance(Circle(), NonRuntimeProto)
    print("non_runtime isinstance", "no error")
except TypeError:
    print("non_runtime isinstance", "TypeError")

# ---------- TypeVar with bounds ----------

T = TypeVar("T")
TNum = TypeVar("TNum", bound=int)
TStrOrBytes = TypeVar("TStrOrBytes", str, bytes)

print("T name", T.__name__)
print("T bound", T.__bound__)
print("T constraints", T.__constraints__)
print("TNum name", TNum.__name__)
print("TNum bound", TNum.__bound__)
print("TNum constraints", TNum.__constraints__)
print("TStrOrBytes name", TStrOrBytes.__name__)
print("TStrOrBytes bound", TStrOrBytes.__bound__)
print("TStrOrBytes constraints", TStrOrBytes.__constraints__)

# ---------- Generic class parameterization ----------

U = TypeVar("U")


class Container(Generic[U]):
    def __init__(self, value: U) -> None:
        self.value = value

    def get(self) -> U:
        return self.value


c = Container(42)
print("container get", c.get())
print("container isinstance", isinstance(c, Container))

# Subscript creates a _GenericAlias
alias = Container[int]
print("Container[int]", alias is not Container)
print("Container[int] origin", get_origin(alias) is Container)
print("Container[int] args", get_args(alias))

# ---------- get_type_hints on annotated class ----------

class Annotated_Class:
    x: int
    y: str = "hello"
    z: "list[int]" = []  # forward ref as string


hints = get_type_hints(Annotated_Class)
print("hints keys", sorted(hints.keys()))
print("hints x", hints["x"])
print("hints y", hints["y"])
print("hints z", hints["z"])

# ---------- Literal ----------

L = Literal["a", "b", 1]
print("Literal origin", get_origin(L))
print("Literal args", get_args(L))

# Nested Literal flattening
L2 = Literal[Literal["x", "y"], "z"]
print("Literal nested args", get_args(L2))

# ---------- Annotated ----------

A = Annotated[int, "metadata", 42]
print("Annotated origin", get_origin(A))
print("Annotated args", get_args(A))

# ---------- Union ----------

U_type = Union[int, str]
print("Union origin", get_origin(U_type))
print("Union args", get_args(U_type))

# Union deduplication
U_dedup = Union[int, int, str]
print("Union dedup args", get_args(U_dedup))

# Union with None is Optional
U_opt = Union[int, None]
print("Union optional args", get_args(U_opt))

# ---------- cast() identity ----------

val = cast(int, "not_an_int")
print("cast identity", val)
print("cast type", type(val).__name__)

val2 = cast(str, 42)
print("cast int as str", val2)
print("cast int as str type", type(val2).__name__)

# ---------- overload decorator ----------

@overload
def process(x: int) -> str: ...

@overload
def process(x: str) -> int: ...

def process(x):
    if isinstance(x, int):
        return str(x)
    return len(x)


print("overload int", process(42))
print("overload str", process("hello"))

# ---------- issubclass with Protocol ----------

print("Circle subclass Drawable", issubclass(Circle, Drawable))
print("Rock subclass Drawable", issubclass(Rock, Drawable))

# ---------- Protocol with multiple methods ----------

@runtime_checkable
class ReadWrite(Protocol):
    def read(self) -> str: ...
    def write(self, data: str) -> None: ...


class FileObj:
    def read(self) -> str:
        return "data"

    def write(self, data: str) -> None:
        pass


class ReadOnly:
    def read(self) -> str:
        return "data"


print("fileobj isinstance ReadWrite", isinstance(FileObj(), ReadWrite))
print("readonly isinstance ReadWrite", isinstance(ReadOnly(), ReadWrite))

# ---------- ClassVar in get_type_hints ----------

class WithClassVar:
    x: int
    y: ClassVar[str] = "class"


hints_cv = get_type_hints(WithClassVar, include_extras=True)
print("classvar hints keys", sorted(hints_cv.keys()))

# ---------- Generic with multiple params ----------

K = TypeVar("K")
V = TypeVar("V")


class Mapping(Generic[K, V]):
    def __init__(self) -> None:
        self.data: dict[K, V] = {}

    def put(self, key: K, val: V) -> None:
        self.data[key] = val

    def get_val(self, key: K) -> V:
        return self.data[key]


m = Mapping()
m.put("a", 1)
print("mapping get", m.get_val("a"))

alias2 = Mapping[str, int]
print("Mapping[str,int] origin", get_origin(alias2) is Mapping)
print("Mapping[str,int] args", get_args(alias2))
