# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: differential coverage for PEP 328 relative imports."""

import importlib


mod = importlib.import_module("rel_pkg.sub.mod_c")
print(mod.VALUE_C)


# Both packages are admitted by the existing guest; metadata changes at actual
# deferred execution and starred namespace escape must keep the transaction
# relative until runtime resolves its current anchor.
from rel_pkg import mod_b as admitted_b

__package__ = "rel_pkg.sub"
pending = ((__package__ := "rel_pkg") for _ in (0,))
assert __package__ == "rel_pkg.sub"
list(pending)
from .mod_b import VALUE_B as generated_value

print("deferred-generator", generated_value == admitted_b.VALUE_B)

__package__ = "rel_pkg"
exposed = globals()


def reanchor():
    exposed["__package__"] = "rel_pkg.sub"


reanchor()
from .mod_c import VALUE_C as captured_value

print("deferred-capture", captured_value == mod.VALUE_C)


def consume(namespace):
    namespace["__package__"] = "rel_pkg"


__package__ = "rel_pkg.sub"
exposed_tuple = (globals(),)
consume(*exposed_tuple)
from .mod_b import VALUE_B as starred_value

print("starred-namespace", starred_value == admitted_b.VALUE_B)
