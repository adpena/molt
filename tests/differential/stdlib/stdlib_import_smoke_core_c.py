"""Purpose: stdlib import smoke for core pure modules (C)."""

import timeit
import token
import tokenize
import quopri
import pickletools
import this

modules = [
    timeit,
    token,
    tokenize,
    quopri,
    pickletools,
    this,
]
print([mod.__name__ for mod in modules])
