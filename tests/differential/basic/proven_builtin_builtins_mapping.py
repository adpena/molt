"""A module that binds ``__builtins__`` changes its functions' builtins (HF-226).

CPython captures a function's builtins from ``globals()["__builtins__"]`` when
the function is created. A writer of that key must keep every builtin read in
the module unproved, so the compiler keeps the live lookup.
"""

import builtins


def before(value):
    return isinstance(value, str), len("abc")


custom = dict(builtins.__dict__)
custom["len"] = lambda obj: -1
custom["isinstance"] = lambda obj, kind: "custom"
__builtins__ = custom


def after(value):
    return isinstance(value, str), len("abc")


print("before", before("a"))
print("after", after("a"))
