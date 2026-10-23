"""Purpose: sys.exit(code) raises SystemExit built as CPython's
_PyErr_SetObject builds it: no arguments for None, the tuple's items for a
tuple, the code itself otherwise; the process exits with the final code."""

import sys

for code in (None, 0, 3, "message", (1, 2), (), [5]):
    try:
        sys.exit(code)
    except SystemExit as exc:
        print(type(code).__name__, repr(exc.code), exc.args)

try:
    sys.exit()
except SystemExit as exc:
    print("no-argument", repr(exc.code), exc.args)

instance = SystemExit(7)
try:
    sys.exit(instance)
except SystemExit as exc:
    print("instance", exc is instance, repr(exc.code), exc.args)

try:
    sys.exit(ValueError("v"))
except SystemExit as exc:
    print("other-exception", type(exc.code).__name__, exc.args[0].args)

sys.exit(4)
