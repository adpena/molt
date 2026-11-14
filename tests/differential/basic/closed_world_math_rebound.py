"""Purpose: this program writes `math`'s namespace (rebinding, deleting and a
computed-name setattr), so no `math.f` call is a closed-world binding: every
site keeps the generic call and must observe each write at its next execution
(call_anatomy.md 4.5, invariant J3), as CPython does. A module `__getattr__`
answers a deleted name."""

import math
import sys

original_sqrt = math.sqrt


def site(x):
    return math.sqrt(x)


def never_called():
    math.sqrt = None


print("before", site(4.0))
math.sqrt = lambda x: ("rebound", x)
print("rebound", site(4.0))
math.sqrt = original_sqrt
print("restored", site(9.0))
del math.sqrt
try:
    site(1.0)
except AttributeError as exc:
    print("deleted", type(exc).__name__)
math.__getattr__ = lambda name: (lambda x: ("module getattr", name, x))
print("getattr", site(2.0))
del math.__getattr__
name = "sq" + "rt"
setattr(math, name, original_sqrt)
print("setattr", site(16.0))
saved = sys.modules["math"]
del sys.modules["math"]
import math as reimported  # noqa: E402

print("reimport", reimported.sqrt(25.0), reimported is saved)
sys.modules["math"] = saved
for value in (-1.0, 0.0):
    try:
        print("log2", math.log2(value))
    except ValueError as exc:
        print("log2 error", exc)
