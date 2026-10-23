"""Purpose: a non-integer SystemExit code is written to sys.stderr with a
newline and the exit status is 1 (CPython's _Py_HandleSystemExit)."""

import sys

sys.stderr = sys.stdout
print("before")
sys.exit("goodbye")
