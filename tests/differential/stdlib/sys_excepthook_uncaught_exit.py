"""Purpose: an exception that escapes the main module reaches sys.excepthook
after sys.last_exc and the legacy last_* names are set, and before the
atexit phase; the exit status is 1 (CPython's PyErr_Print)."""

import atexit
import sys


def hook(exc_type, exc, tb):
    print("hook", exc_type.__name__, exc)
    print("last_exc", sys.last_exc is exc)
    print("last_type", sys.last_type is exc_type, "last_value", sys.last_value is exc)
    print("last_traceback", sys.last_traceback is tb, tb is not None)


sys.excepthook = hook
atexit.register(lambda: print("atexit"))
print("main")
raise ValueError("boom")
