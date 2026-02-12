"""Purpose: stdlib import smoke for profiler helpers."""

import profile
import cProfile
import pstats

modules = [
    profile,
    cProfile,
    pstats,
]
print([mod.__name__ for mod in modules])
