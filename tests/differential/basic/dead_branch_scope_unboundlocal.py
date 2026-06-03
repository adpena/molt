"""Purpose: a name bound only in a statically-dead branch is still a function
local (CPython symbol-table semantics), so reading it raises UnboundLocalError,
not NameError. Molt constant-folds dead `if` branches for codegen/emission, but
that fold must NOT prune scope bindings — binding analysis mirrors CPython's
symtable, which records every assignment target regardless of static
reachability. Module scope has no locals, so the same shape there is NameError.
"""

from typing import TYPE_CHECKING


def exc(fn):
    try:
        return fn()
    except UnboundLocalError:
        return "UnboundLocalError"
    except NameError:
        return "NameError"


# `if 0:` — x is a local of f, never assigned at runtime -> UnboundLocalError.
def f_int():
    if 0:
        x = 1
    return x


# `if False:` — same.
def f_false():
    if False:
        y = 1
    return y


# `if "":` — falsy str literal, dead branch, z is still local.
def f_str():
    if "":
        z = 1
    return z


# `if ():` — empty tuple is falsy and a compile-time constant.
def f_tuple():
    if ():
        w = 1
    return w


# `if TYPE_CHECKING:` — statically False under Molt (it never type-checks).
def f_typecheck():
    if TYPE_CHECKING:
        tc = 1
    return tc


# Dead branch nested inside a statically-live branch: inner name is still local.
def f_nested():
    if 1:
        if 0:
            n = 1
    return n


# Live constant branch: x IS assigned at runtime -> returns the value.
def f_live():
    if 1:
        v = 7
    return v


# Dead-branch binding shadowed by a later unconditional assignment: no error.
def f_later_assign():
    if 0:
        a = 1
    a = 99
    return a


print("f_int       ", exc(f_int))
print("f_false     ", exc(f_false))
print("f_str       ", exc(f_str))
print("f_tuple     ", exc(f_tuple))
print("f_typecheck ", exc(f_typecheck))
print("f_nested    ", exc(f_nested))
print("f_live      ", f_live())
print("f_later     ", f_later_assign())


# Module scope: no locals; a dead-branch global is simply never set -> NameError.
if 0:
    module_only = 1
try:
    print("module      ", module_only)
except NameError:
    print("module      ", "NameError")
