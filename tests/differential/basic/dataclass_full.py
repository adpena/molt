"""Purpose: differential coverage for full dataclass surface."""

from dataclasses import (
    KW_ONLY,
    InitVar,
    asdict,
    astuple,
    dataclass,
    field,
    fields,
    is_dataclass,
    replace,
)
from typing import ClassVar


@dataclass
class Base:
    x: int
    y: int = 2


@dataclass
class Child(Base):
    z: int = 3


print(Child(1))
print(Child(1).x, Child(1).y, Child(1).z)


@dataclass
class EqTest:
    a: int
    b: int = field(compare=False, repr=False)


print(EqTest(1, 2) == EqTest(1, 99))
print(EqTest(1, 2) == EqTest(2, 2))
print(EqTest(1, 2))


@dataclass(order=True)
class Ord:
    a: int
    b: int = field(compare=False)


print(Ord(1, 100) < Ord(2, 0))
print(Ord(1, 100) < Ord(1, 0))


@dataclass(frozen=True)
class Frozen:
    a: int
    b: int = field(hash=False)


f = Frozen(1, 2)
print(hash(f) == hash(Frozen(1, 3)))
object.__setattr__(f, "a", 5)
print(f.a)


@dataclass
class Sample:
    a: int
    b: int = field(repr=False, compare=False)
    c: list = field(default_factory=list)
    d: int = field(init=False, default=5)
    e: InitVar[int] = 7
    _: KW_ONLY
    k: int
    m: int = 9
    cv: ClassVar[int] = 11

    def __post_init__(self, e):
        self.c.append(e)


s = Sample(1, 2, k=4)
print(s)
print(s.a, s.b, s.c, s.d, s.k, s.m, Sample.cv)
print(Sample.__match_args__)
print([f.name for f in fields(Sample)])
print(asdict(s))
print(astuple(s))
print(is_dataclass(s), is_dataclass(Sample), is_dataclass(3))
print(replace(s, a=10, k=8, e=4).c)
