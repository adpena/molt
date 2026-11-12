"""Purpose: every writer of a module namespace reaches the next read of the
global at a compiled site (call_anatomy.md 4.5, invariant J3). Module globals
are mirrored in per-module slot tables; each section writes a global through
one path (STORE_GLOBAL at module level and in a function, globals(),
module.__dict__, setattr, del with a builtin fallback, import, reload, module
__getattr__, __class__ reassignment, a foreign-globals function alias and a
non-str key) and reads it again at a site that already ran."""
# ruff: noqa: E402

import importlib
import os
import sys
import types

sys.path.insert(0, os.path.dirname(__file__))

import module_global_slots_mod as helper

this = sys.modules[__name__]
value = 0


def read_value():
    return value


def store_value(new):
    global value
    value = new


def read_len_name():
    return len


trace = []
for step in range(6):
    trace.append(read_value())
    if step == 0:
        value = 1  # STORE_GLOBAL at module level
    elif step == 1:
        store_value(2)  # STORE_GLOBAL in a function
    elif step == 2:
        globals()["value"] = 3
    elif step == 3:
        this.__dict__["value"] = 4
    elif step == 4:
        setattr(this, "value", 5)
print("writers", trace, read_value(), value)

# del falls back to the builtin; rebinding shadows it again.
len = "shadow"
print("shadowed", read_len_name())
del len
print("builtin", read_len_name()("abc"))
globals()["len"] = "shadow2"
print("shadowed again", read_len_name())
del globals()["len"]
print("builtin again", read_len_name()("abcd"))

# A deleted global raises NameError at the next read.
del value
try:
    read_value()
except NameError as exc:
    # str(exc) carries Molt's traceback suggestion today (a separate parity
    # defect); the exception type and name are the facts under test.
    print("deleted", type(exc).__name__, exc.args[0].split(".")[0])
value = 7
print("rebound", read_value())

# Another module's globals, written from here and read by its own functions.
seen = []
for step in range(4):
    seen.append((helper.read_counter(), helper.counter, helper.read_label()))
    if step == 0:
        helper.bump()
    elif step == 1:
        helper.counter = 10
        setattr(helper, "label", "second")
    elif step == 2:
        helper.__dict__["counter"] = 20
print("helper", seen)
del helper.label
try:
    helper.read_label()
except NameError as exc:
    print("helper deleted", type(exc).__name__, exc.args[0].split(".")[0])
print("getattr", helper.dynamic)
helper.dynamic = "bound"
print("getattr bound", helper.dynamic)
del helper.dynamic
print("getattr again", helper.dynamic)

# reload re-executes the body into the same namespace.
helper.counter = 99
helper = importlib.reload(helper)
print("reloaded", helper.counter, helper.read_counter(), helper.read_label())


# A ModuleType subclass with a property shadows the namespace entry.
class Special(types.ModuleType):
    @property
    def counter(self):
        return "property"


helper.__class__ = Special
print("subclass", helper.counter, helper.read_counter())
helper.__class__ = types.ModuleType
print("plain again", helper.counter)

# A function alias with foreign globals reads its own namespace.
foreign = {"value": "foreign", "__builtins__": __builtins__}
alias = types.FunctionType(read_value.__code__, foreign)
print("alias", alias(), read_value())
foreign["value"] = "foreign2"
print("alias again", alias(), read_value())

# A key that is not an exact str keeps every read correct.
globals()[3] = "int key"
value = 8
print("non-str key", read_value(), globals()[3])


class Key(str):
    pass


globals()[Key("value")] = 9
print("str subclass key", read_value(), value)
