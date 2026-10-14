"""Shadow len, int and isinstance through every namespace path (HF-79).

Molt specializes a builtin call in a function behind a guard on the module
and builtins namespaces. Guard bits never clear, so a path switches a call
from the specialized arm to the generic arm only if it is the first to touch
that name in that namespace. Each run therefore pairs every name with one
module path and one builtins path, and the three rotation files pair each
name with each path once. CPython's output is the oracle.

Molt has no ``exec`` in compiled binaries (the dynamic-execution policy), so
no compiled code can rebind a name through it.
"""

import builtins

from builtin_binding_guard_support import guarded

NAMES = ("len", "int", "isinstance")
ORIGINALS = {name: getattr(builtins, name) for name in NAMES}


def fake_len(value):
    return -1


def fake_isinstance(value, kind):
    return "shadowed"


# A shadow of `int` changes both `int(text)` and the class operand of
# `isinstance(value, int)`.
SHADOWS = {"len": fake_len, "int": float, "isinstance": fake_isinstance}


# Probes for the builtins paths. This module never binds the three names, so
# its own reads are guarded as well.
def builtins_size(value):
    return len(value)


def builtins_to_int(text):
    return int(text)


def builtins_is_int(value):
    return isinstance(value, int)


def guarded_probe(name):
    if name == "len":
        return guarded.size([1, 2, 3])
    if name == "int":
        return guarded.to_int("7"), guarded.is_int(5)
    return guarded.is_int(5)


def builtins_probe(name):
    if name == "len":
        return builtins_size([1, 2, 3])
    if name == "int":
        return builtins_to_int("7"), builtins_is_int(5)
    return builtins_is_int(5)


def observe(probe, name):
    try:
        return probe(name)
    except NameError as error:
        return type(error).__name__


def module_setattr(name, shadow):
    setattr(guarded, name, shadow)
    return lambda: delattr(guarded, name)


def module_dict(name, shadow):
    guarded.__dict__[name] = shadow
    return lambda: guarded.__dict__.pop(name)


def module_globals(name, shadow):
    guarded.bind(name, shadow)
    return lambda: guarded.unbind(name)


def builtins_setattr(name, shadow):
    setattr(builtins, name, shadow)
    return lambda: setattr(builtins, name, ORIGINALS[name])


def builtins_dict(name, shadow):
    builtins.__dict__[name] = shadow
    return lambda: builtins.__dict__.update({name: ORIGINALS[name]})


def builtins_del(name, shadow):
    delattr(builtins, name)
    return lambda: setattr(builtins, name, ORIGINALS[name])


MODULE_PATHS = (module_setattr, module_dict, module_globals)
BUILTINS_PATHS = (builtins_setattr, builtins_dict, builtins_del)


def shadow_cell(path, probe, name):
    before = observe(probe, name)
    restore = path(name, SHADOWS[name])
    during = observe(probe, name)
    restore()
    after = observe(probe, name)
    print(path.__name__, name, before, during, after)


def run(rotation):
    # Module paths first: a builtins bit would fail the guard on its own and
    # hide whether the module path switched it.
    for index, name in enumerate(NAMES):
        shadow_cell(MODULE_PATHS[(index + rotation) % 3], guarded_probe, name)
    for index, name in enumerate(NAMES):
        shadow_cell(BUILTINS_PATHS[(index + rotation) % 3], builtins_probe, name)
    # Every other cell now runs on the generic arm; its semantics still hold.
    for path in MODULE_PATHS:
        for name in NAMES:
            shadow_cell(path, guarded_probe, name)
    for path in BUILTINS_PATHS:
        for name in NAMES:
            shadow_cell(path, builtins_probe, name)
    # Clearing and refilling the namespace rebinds through the reset path.
    saved = dict(guarded.__dict__)
    guarded.__dict__.clear()
    guarded.__dict__.update(saved)
    guarded.__dict__.update({"len": fake_len})
    print("module-clear-refill", guarded_probe("len"))
    guarded.__dict__.pop("len")
    print("module-clear-restored", guarded_probe("len"), guarded_probe("int"))
    assert all(getattr(builtins, name) is ORIGINALS[name] for name in NAMES)
