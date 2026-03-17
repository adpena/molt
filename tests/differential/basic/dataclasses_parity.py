"""Purpose: differential coverage for dataclasses — edge cases beyond dataclass_full.py."""

from dataclasses import (
    FrozenInstanceError,
    InitVar,
    asdict,
    astuple,
    dataclass,
    field,
    fields,
    is_dataclass,
    make_dataclass,
    replace,
)
from typing import ClassVar

# ---------- Basic dataclass with various field types ----------

@dataclass
class Config:
    name: str
    count: int
    ratio: float
    active: bool
    tags: list = field(default_factory=list)


cfg = Config("test", 10, 3.14, True)
print("config", cfg)
print("config name", cfg.name)
print("config tags", cfg.tags)
print("config eq", cfg == Config("test", 10, 3.14, True))
print("config neq", cfg == Config("test", 10, 3.14, False))

# ---------- field() with default, default_factory, repr, compare, hash ----------

@dataclass
class FieldOpts:
    visible: int
    hidden: int = field(repr=False)
    no_compare: int = field(default=0, compare=False)
    factory: list = field(default_factory=lambda: [1, 2, 3])


fo = FieldOpts(10, 20)
print("fieldopts repr", fo)
print("fieldopts eq ignore no_compare", FieldOpts(10, 20, 0) == FieldOpts(10, 20, 999))
print("fieldopts eq hidden matters", FieldOpts(10, 20) == FieldOpts(10, 30))
print("fieldopts factory", fo.factory)

# Verify separate instances get separate lists
fo2 = FieldOpts(10, 20)
fo.factory.append(4)
print("factory isolation", fo2.factory)

# ---------- frozen=True — FrozenInstanceError on assignment ----------

@dataclass(frozen=True)
class FrozenPoint:
    x: int
    y: int


fp = FrozenPoint(1, 2)
print("frozen repr", fp)
print("frozen eq", fp == FrozenPoint(1, 2))
print("frozen hash", hash(fp) == hash(FrozenPoint(1, 2)))
print("frozen hash diff", hash(fp) != hash(FrozenPoint(3, 4)))

try:
    fp.x = 10
    print("frozen assign", "no error")
except FrozenInstanceError:
    print("frozen assign", "FrozenInstanceError")

try:
    del fp.x
    print("frozen delete", "no error")
except FrozenInstanceError:
    print("frozen delete", "FrozenInstanceError")

# Frozen as dict key
d = {fp: "origin"}
print("frozen dict lookup", d[FrozenPoint(1, 2)])

# ---------- order=True — comparison operators ----------

@dataclass(order=True)
class Version:
    major: int
    minor: int
    patch: int


v1 = Version(1, 0, 0)
v2 = Version(1, 2, 0)
v3 = Version(2, 0, 0)
v4 = Version(1, 0, 0)

print("order lt", v1 < v2)
print("order gt", v3 > v2)
print("order le", v1 <= v4)
print("order ge", v2 >= v1)
print("order eq", v1 == v4)
print("order sorted", [str(v) for v in sorted([v3, v1, v2])])

# Order with compare=False fields
@dataclass(order=True)
class Priority:
    level: int
    label: str = field(compare=False)


print("order skip field", Priority(1, "low") < Priority(2, "high"))
print("order skip eq", Priority(1, "low") == Priority(1, "different"))

# ---------- asdict and astuple with nested dataclasses ----------

@dataclass
class Inner:
    a: int
    b: str


@dataclass
class Outer:
    inner: Inner
    value: int


o = Outer(Inner(1, "x"), 42)
d_out = asdict(o)
print("asdict nested", d_out)
print("asdict inner type", type(d_out["inner"]).__name__)

t_out = astuple(o)
print("astuple nested", t_out)
print("astuple inner type", type(t_out[0]).__name__)

# asdict with list of dataclasses
@dataclass
class Container:
    items: list


cont = Container([Inner(1, "a"), Inner(2, "b")])
d_cont = asdict(cont)
print("asdict list", d_cont)
print("asdict list item type", type(d_cont["items"][0]).__name__)

# ---------- fields() introspection ----------

@dataclass
class Introspect:
    x: int
    y: str = "default"
    z: list = field(default_factory=list, repr=False)


from dataclasses import MISSING

fs = fields(Introspect)
print("fields count", len(fs))
for f in fs:
    has_default = f.default is not MISSING
    has_factory = f.default_factory is not MISSING
    if has_default:
        default_str = repr(f.default)
    elif has_factory:
        default_str = "FACTORY"
    else:
        default_str = "MISSING"
    print(f"field {f.name}: type={f.type.__name__}, repr={f.repr}, default={default_str}")

# fields on instance vs class
inst = Introspect(1)
print("fields instance same", [f.name for f in fields(inst)] == [f.name for f in fields(Introspect)])

# ---------- replace() ----------

@dataclass
class RGB:
    r: int
    g: int
    b: int


color = RGB(255, 0, 0)
blue = replace(color, b=255, r=0)
print("replace result", blue)
print("replace original unchanged", color)

# replace on frozen
@dataclass(frozen=True)
class FrozenRGB:
    r: int
    g: int
    b: int


fc = FrozenRGB(255, 0, 0)
fc2 = replace(fc, g=128)
print("replace frozen", fc2)
print("replace frozen original", fc)

# ---------- __post_init__ ----------

@dataclass
class Validated:
    name: str
    value: int
    processed: str = field(init=False)

    def __post_init__(self):
        self.processed = f"{self.name}:{self.value}"
        if self.value < 0:
            raise ValueError("negative")


v = Validated("x", 42)
print("post_init processed", v.processed)

try:
    Validated("y", -1)
    print("post_init validation", "no error")
except ValueError as e:
    print("post_init validation", str(e))

# ---------- __post_init__ with InitVar ----------

@dataclass
class WithInitVar:
    base: int
    multiplier: InitVar[int]
    result: int = field(init=False)

    def __post_init__(self, multiplier: int):
        self.result = self.base * multiplier


wi = WithInitVar(5, 3)
print("initvar result", wi.result)
print("initvar base", wi.base)

# InitVar should not appear in fields()
print("initvar fields", [f.name for f in fields(wi)])

# InitVar should not appear in repr
print("initvar repr", wi)

# ---------- Inheritance ----------

@dataclass
class Animal:
    name: str
    legs: int


@dataclass
class Dog(Animal):
    breed: str


d = Dog("Rex", 4, "Labrador")
print("inherit repr", d)
print("inherit fields", [f.name for f in fields(d)])
print("inherit isinstance", isinstance(d, Animal))

# Override field in child
@dataclass
class Base:
    x: int = 0
    y: int = 0


@dataclass
class Child(Base):
    y: int = 10
    z: int = 20


ch = Child(1)
print("inherit override", ch)
print("inherit override y", ch.y)
print("inherit override z", ch.z)

# ---------- Empty dataclass ----------

@dataclass
class Empty:
    pass


e = Empty()
print("empty repr", e)
print("empty eq", e == Empty())
print("empty fields", len(fields(e)))
print("empty is_dataclass", is_dataclass(e))

# ---------- Dataclass with ClassVar (should be excluded from fields) ----------

@dataclass
class WithCV:
    x: int
    class_var: ClassVar[int] = 99


wcv = WithCV(1)
print("classvar fields", [f.name for f in fields(wcv)])
print("classvar access", WithCV.class_var)
print("classvar repr", wcv)

# ---------- Dataclass with __eq__ override respected ----------

@dataclass
class CustomEq:
    value: int

    def __eq__(self, other):
        return True  # always equal


print("custom eq", CustomEq(1) == CustomEq(2))
print("custom eq non-dc", CustomEq(1) == "anything")

# ---------- field(init=False) ----------

@dataclass
class AutoField:
    name: str
    computed: str = field(init=False, default="auto")


af = AutoField("test")
print("init_false", af.computed)
print("init_false repr", af)

# ---------- hash behavior ----------

# Default: __hash__ is None when eq=True (unhashable)
@dataclass
class Unhashable:
    x: int


try:
    hash(Unhashable(1))
    print("default hash", "hashable")
except TypeError:
    print("default hash", "TypeError")

# unsafe_hash=True forces hash
@dataclass(unsafe_hash=True)
class UnsafeHash:
    x: int


print("unsafe_hash", hash(UnsafeHash(1)) == hash(UnsafeHash(1)))

# frozen implies hash
print("frozen hashable", hash(FrozenPoint(1, 2)) == hash(FrozenPoint(1, 2)))

# ---------- make_dataclass dynamic creation ----------

Point3D = make_dataclass("Point3D", [("x", float), ("y", float), ("z", float)], frozen=True)
p3 = Point3D(1.0, 2.0, 3.0)
print("make_dc repr", p3)
print("make_dc frozen", hash(p3) == hash(Point3D(1.0, 2.0, 3.0)))

# ---------- slots=True ----------

@dataclass(slots=True)
class Slotted:
    x: int
    y: int


sl = Slotted(1, 2)
print("slots repr", sl)
print("slots has __slots__", hasattr(Slotted, "__slots__"))
print("slots values", sl.x, sl.y)

try:
    sl.z = 3  # type: ignore
    print("slots dynamic attr", "no error")
except AttributeError:
    print("slots dynamic attr", "AttributeError")

# ---------- match_args ----------

@dataclass
class MatchArgs:
    a: int
    b: str
    c: float = 0.0


print("match_args", MatchArgs.__match_args__)

@dataclass(match_args=False)
class NoMatchArgs:
    a: int


print("no match_args", hasattr(NoMatchArgs, "__match_args__"))
