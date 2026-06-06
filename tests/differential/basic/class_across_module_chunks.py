"""Regression for task #50 — class instantiated in a LATER module chunk than its definition.

When the module body (`molt_main`) exceeds the native chunk-op budget it is split
into several `molt_module_chunk_N` functions (native default 1400 ops,
src/molt/cli.py; WASM default 2000; override via the `MOLT_MODULE_CHUNK_OPS`
environment variable). A class defined in chunk N and instantiated in chunk N+M
must re-resolve its class object through a chunk-safe lookup: at the chunk boundary
`_reset_module_chunk_state` clears `self.globals`/`self.locals`, so the class' SSA
value from the earlier chunk is no longer live. The `molt_main` constructor-fold
fast path (src/molt/frontend/visitors/calls.py) previously reused the stale
`class_value_name` SSA reference with no liveness check; lowering degraded it to a
`CONST_STR` of the variable name (e.g. literal "v312"), and the runtime then saw a
string where a type was expected — either a SIGSEGV (direct-field-store fast path
dereferencing the None error sentinel) or a spurious
`TypeError: object.__new__ expects type`. The fix routes that branch through the
single liveness-guarded resolver `_current_module_static_class_ref`, falling back to
MODULE_GET_ATTR when the class SSA value is not live in the current chunk.

This file is intentionally large enough (many op-dense early class definitions) to
force a >=2-chunk split at the DEFAULT native threshold, so it reproduces the bug
WITHOUT any environment override and is a self-contained CI regression. To also
exercise smaller programs deterministically, build/run with a tight lever, e.g.:

    MOLT_MODULE_CHUNK_OPS=50 python3 -m molt build --target native --output /tmp/o this_file.py

Covers plain, PEP 695 generic, and @dataclass classes through the same
constructor path — all three must instantiate correctly across the chunk boundary.
Runs byte-identical to CPython 3.12/3.13/3.14.
"""

from dataclasses import dataclass


# --- PEP 695 generic classes (op-dense: type params + annotated bodies) -------
class GBox0[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class GBox1[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class GBox2[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


class GBox3[T]:
    def __init__(self, value: T):
        self.value: T = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> T:
        old = self.value
        self.value = new
        return old


# --- plain (non-generic) classes ---------------------------------------------
class PBox0:
    def __init__(self, value):
        self.value = value

    def get(self):
        return self.value

    def replace(self, new):
        old = self.value
        self.value = new
        return old


class PBox1:
    def __init__(self, value):
        self.value = value

    def get(self):
        return self.value

    def replace(self, new):
        old = self.value
        self.value = new
        return old


class PBox2:
    def __init__(self, value):
        self.value = value

    def get(self):
        return self.value

    def replace(self, new):
        old = self.value
        self.value = new
        return old


class PBox3:
    def __init__(self, value):
        self.value = value

    def get(self):
        return self.value

    def replace(self, new):
        old = self.value
        self.value = new
        return old


# --- @dataclass classes -------------------------------------------------------
@dataclass
class DPoint0:
    x: int
    y: int

    def total(self) -> int:
        return self.x + self.y


@dataclass
class DPoint1:
    x: int
    y: int

    def total(self) -> int:
        return self.x + self.y


@dataclass
class DPoint2:
    x: int
    y: int

    def total(self) -> int:
        return self.x + self.y


@dataclass
class DPoint3:
    x: int
    y: int

    def total(self) -> int:
        return self.x + self.y


if __name__ == "__main__":
    # Instantiations happen far below the definitions; with the module split into
    # multiple chunks these land in a later chunk than the class defs.
    g0 = GBox0(0)
    print("g0", g0.get(), g0.replace(0 + 1))
    g1 = GBox1("a")
    print("g1", g1.get(), g1.replace("b"))
    g2 = GBox2(2)
    print("g2", g2.get(), g2.replace(2 + 1))
    g3 = GBox3([3])
    print("g3", g3.get(), g3.replace([4]))

    p0 = PBox0(10)
    print("p0", p0.get(), p0.replace(11))
    p1 = PBox1("x")
    print("p1", p1.get(), p1.replace("y"))
    p2 = PBox2(12)
    print("p2", p2.get(), p2.replace(13))
    p3 = PBox3((1, 2))
    print("p3", p3.get(), p3.replace((3, 4)))

    d0 = DPoint0(1, 2)
    print("d0", d0.x, d0.y, d0.total())
    d1 = DPoint1(3, 4)
    print("d1", d1.x, d1.y, d1.total())
    d2 = DPoint2(5, 6)
    print("d2", d2.x, d2.y, d2.total())
    d3 = DPoint3(7, 8)
    print("d3", d3.x, d3.y, d3.total())

    print("OK")
