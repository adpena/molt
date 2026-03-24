"""Purpose: differential coverage for @dataclass(**OPTS) compile-time resolution."""

from dataclasses import dataclass

SLOTS = {"slots": True}


@dataclass(**SLOTS)
class Point:
    x: int
    y: int


p = Point(1, 2)
print(p)
print(p.x, p.y)

# Verify slots are actually set
print(hasattr(Point, "__slots__"))

# Multiple options via **kwargs
DC_OPTS = {"frozen": True, "eq": True}


@dataclass(**DC_OPTS)
class Frozen:
    value: int


f = Frozen(42)
print(f)
print(f.value)

# Frozen should reject mutation
try:
    f.value = 99  # type: ignore
    print("ERROR: mutation allowed on frozen")
except AttributeError:
    print("frozen-ok")
except Exception as e:
    print(f"frozen-error: {type(e).__name__}")

# Direct kwargs still work alongside
BASIC = {"repr": True}


@dataclass(**BASIC, eq=True)
class Mixed:
    name: str


m = Mixed("hello")
print(m)

# Version-gated pattern (common in real code)
import sys

if sys.version_info >= (3, 10):
    _OPTS = {"slots": True}
else:
    _OPTS = {}


@dataclass(**_OPTS)
class Versioned:
    tag: str


v = Versioned("v1")
print(v)
print(v.tag)
