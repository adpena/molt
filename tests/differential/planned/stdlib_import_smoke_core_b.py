"""Purpose: stdlib import smoke for core pure modules (B)."""

import difflib
import graphlib
import optparse
import reprlib
import sched
import stringprep

modules = [
    difflib,
    graphlib,
    optparse,
    reprlib,
    sched,
    stringprep,
]
print([mod.__name__ for mod in modules])
