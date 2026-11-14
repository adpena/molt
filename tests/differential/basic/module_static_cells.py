"""Purpose: module globals held in static cells (call_anatomy.md 4.5a). No
code in this program reaches a module dictionary by identity, so this module
and its helper keep their globals in fixed-address cells; every by-name path
(attribute access and mutation, del, dir, hasattr, getattr with a computed
name, module __getattr__, pickle's find_class, an import cycle) must observe
the cells exactly as a dictionary namespace."""
# ruff: noqa: E402

import os
import pickle
import sys

sys.path.insert(0, os.path.dirname(__file__))

import module_static_cells_mod as helper

counter = 0


def bump():
    global counter
    counter += 1


for _ in range(3):
    bump()
print("counter", counter)

# del then NameError, at module level and in a function.
victim = 1
del victim
try:
    print(victim)
except NameError as exc:
    print("deleted", type(exc).__name__)


def read_victim():
    return victim


try:
    read_victim()
except NameError as exc:
    print("deleted in function", type(exc).__name__)
victim = 2
print("rebound", read_victim())

# Foreign writes M.x = v, setattr and del reach the helper's own reads.
print("helper before", helper.value, helper.read_value())
helper.value = 10
print("helper after", helper.value, helper.read_value())
setattr(helper, "value", 11)
print("helper setattr", helper.read_value())
del helper.value
try:
    helper.read_value()
except NameError as exc:
    print("helper deleted", type(exc).__name__)
print("helper attr after del", helper.value)
helper.value = 12

# A missing name reaches the module __getattr__ hook.
print("getattr hook", helper.missing_name)

# Computed names read the cells.
for name in ["value", "label", "read_value"]:
    attr = getattr(helper, name)
    print("computed", name, attr() if callable(attr) else attr)
print("hasattr", hasattr(helper, "label"), hasattr(helper, "nothing_here"))
print("dir", [name for name in dir(helper) if not name.startswith("_")])
print("own dir", [name for name in dir(sys.modules[__name__]) if name == "counter"])

# pickle's find_class resolves a function by module and name.
restored = pickle.loads(pickle.dumps(helper.read_label))
print("pickle", restored is helper.read_label, restored())

# An import cycle reads a global before its module stores it.
import module_static_cells_cycle_a as cycle_a

print("cycle", cycle_a.seen, cycle_a.after)


# A global that shadows a builtin, then its deletion.
def use_len():
    return len("abc")


print("builtin len", use_len())
len = lambda text: "shadow"  # noqa: E731
print("shadowed len", use_len())
del len
print("builtin again", use_len())
