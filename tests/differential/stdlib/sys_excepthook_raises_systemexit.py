"""Purpose: a SystemExit raised by sys.excepthook sets the exit status, and
the atexit phase still runs (CPython's PyErr_Print)."""

import atexit
import sys


def hook(exc_type, exc, tb):
    print("hook", exc_type.__name__, exc)
    raise SystemExit(7)


sys.excepthook = hook
atexit.register(lambda: print("atexit"))
raise KeyError("k")
