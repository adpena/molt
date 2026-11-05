"""A replaced `type` and `ord` reach calls in functions (HF-776).

Molt lowered both calls by name with no proof, so it printed
`replaced <class 'int'> 97` where CPython prints `replaced replaced-type -1`.
"""

import builtins

original_type = builtins.type
original_ord = builtins.ord


def classify(value):
    return type(value)


def code(text):
    return ord(text)


builtins.type = lambda value: "replaced-type"
builtins.ord = lambda text: -1
print("replaced", classify(1), code("a"))
builtins.type = original_type
builtins.ord = original_ord
print("restored", classify(1).__name__, code("a"))
