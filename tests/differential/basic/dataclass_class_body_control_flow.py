"""Purpose: @dataclass whose body needs block execution (P0 #50).

A dataclass body that contains control flow (for/if/while/del) forces the #50
class-body re-lower onto the block-execution (``dynamic_build``) path.  The
``@dataclass`` runtime application — which installs the generated ``__init__`` /
``__repr__`` / ``__eq__`` / ``__hash__`` / frozen guards — must still run on that
path.  Before the fix, the dataclass-application emission was gated under
``if not dynamic_build:`` and was silently skipped, so the class got a bare
``object.__init__`` (``Cfg() takes no arguments``) and the default object repr.

Every class below mixes dataclass fields with class-scope control flow.
"""

from dataclasses import dataclass


@dataclass
class Cfg:
    n: int = 3
    total: int = 0
    for _i in range(3):  # for-loop at class scope -> total computed before fields finalize
        total = total + _i


@dataclass
class Picked:
    label: str = "x"
    weight: int = 0
    if weight == 0:
        label = "zero"
    else:
        label = "nonzero"


@dataclass
class Counter:
    count: int = 0
    seen: int = 0
    while seen < 5:
        count += seen
        seen += 1


@dataclass(frozen=True)
class FrozenCfg:
    a: int = 1
    b: int = 2
    for _k in range(2):
        b += _k


c = Cfg()
print("Cfg", c.n, c.total)
print("Cfg repr", repr(c))
print("Cfg eq", Cfg() == Cfg(3, 3))
print("Cfg ctor", Cfg(10, 20).n, Cfg(10, 20).total)

p = Picked()
print("Picked", p.label, p.weight)
print("Picked repr", repr(p))

cnt = Counter()
print("Counter", cnt.count, cnt.seen)
print("Counter eq", Counter() == Counter(10, 5))

f = FrozenCfg()
print("Frozen", f.a, f.b)
print("Frozen repr", repr(f))
print("Frozen hash eq", hash(f) == hash(FrozenCfg(1, 3)))
try:
    f.a = 99
    print("Frozen MUTATED")
except Exception as exc:
    print("Frozen", type(exc).__name__)
