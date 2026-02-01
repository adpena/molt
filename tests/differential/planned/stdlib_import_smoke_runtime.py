"""Purpose: stdlib import smoke for runtime helpers."""

import marshal
import faulthandler
import tracemalloc
import pyexpat

modules = [
    marshal,
    faulthandler,
    tracemalloc,
    pyexpat,
]
print([mod.__name__ for mod in modules])
