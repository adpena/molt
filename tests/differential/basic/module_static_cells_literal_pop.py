"""Purpose: a module-scope ``globals().pop("name", <constant>)`` statement is a
tolerant delete of a module global (HF-844). It keeps the module in static
cells, deletes a bound name, ignores an unbound one, and leaves a stdlib
module without the helper it pops."""

import math
import os
import typing

keep = "kept"
dropped = "dropped"
globals().pop("dropped", None)
globals().pop("never_bound", 0)

try:
    print(dropped)
except NameError as exc:
    print("popped", type(exc).__name__)
print("kept", keep)


def read_dropped():
    return dropped


try:
    read_dropped()
except NameError as exc:
    print("popped in function", type(exc).__name__)

dropped = "again"
print("rebound", read_dropped())

if keep:
    globals().pop("dropped", None)
try:
    print(dropped)
except NameError as exc:
    print("conditional pop", type(exc).__name__)

# Stdlib modules pop their intrinsic loader at the end of their body.
print("math loader", hasattr(math, "_require_intrinsic"))
print("os typing helper", hasattr(os, "TYPE_CHECKING"))
print("typing loader", hasattr(typing, "_require_intrinsic"))
print("math works", math.sqrt(16.0), math.floor(2.5))
print("typing works", typing.get_origin(typing.List[int]) is list)
