"""Purpose: a module whose dictionary code reaches by identity keeps its
dictionary namespace (call_anatomy.md 4.5a): globals(), vars(module) and
module.__dict__ reads and writes see every global, and compiled reads see
every dictionary write."""
# ruff: noqa: E402

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import module_static_cells_reflected_mod as reflected

total = 0
for step in range(3):
    total += step
print("globals read", globals()["total"], sorted(k for k in globals() if k == "total"))
globals()["total"] = 100
print("globals write", total)

print("vars read", vars(reflected)["value"], reflected.read_value())
vars(reflected)["value"] = 2
print("vars write", reflected.value, reflected.read_value())
reflected.__dict__["value"] = 3
print("dict write", reflected.read_value())
reflected.value = 4
print("dict sees attribute write", reflected.__dict__["value"])
del reflected.__dict__["value"]
try:
    reflected.read_value()
except NameError as exc:
    print("dict delete", type(exc).__name__)
