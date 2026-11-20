"""String annotations resolve dataclass markers through the class's module.

With postponed evaluation every annotation is a string, so `dataclass` reads
the defining module's namespace to tell `ClassVar`, `InitVar` and `KW_ONLY`
from ordinary fields, bare or module-qualified, through any alias the module
binds. The lookup is `module.__dict__.get`: a name the module does not bind
is an ordinary field, never a builtin or a module `__getattr__` result.
"""

from __future__ import annotations

import dataclasses
import dataclasses as dc
import typing
from dataclasses import KW_ONLY, InitVar, dataclass, field, fields
from typing import ClassVar

LIMIT = 10


@dataclass
class Bare:
    count: int
    shared: ClassVar[int] = 3
    seed: InitVar[int] = 0
    total: int = field(default=0)

    def __post_init__(self, seed):
        self.total = self.count + seed + LIMIT


@dataclass
class Qualified:
    name: str
    shared: typing.ClassVar[str] = "s"
    seed: dataclasses.InitVar[int] = 1
    alias_seed: dc.InitVar[int] = 2
    tags: list = field(default_factory=list)

    def __post_init__(self, seed, alias_seed):
        self.tags = [seed, alias_seed]


@dataclass
class KeywordOnly:
    first: int
    _: KW_ONLY
    second: int = 5


@dataclass
class Unbound:
    # Neither name is bound in this module: both stay ordinary fields.
    marker: Missing = None
    other: NotDefinedHere.Thing = None


for cls in (Bare, Qualified, KeywordOnly, Unbound):
    print(cls.__name__, [f.name for f in fields(cls)])

print("bare", Bare(4, 2), Bare.shared)
print("qualified", Qualified("q", 7, 8), Qualified.shared)
print("kw", KeywordOnly(1, second=9))
try:
    KeywordOnly(1, 2)
except TypeError as exc:
    print("kw positional", type(exc).__name__)
print("unbound", Unbound())
print("limit", LIMIT)
