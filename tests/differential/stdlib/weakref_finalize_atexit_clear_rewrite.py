"""Purpose: weakref.finalize shutdown callbacks survive atexit._clear rewrites."""

import atexit
import weakref


class Box:
    pass


live = Box()
weakref.finalize(live, print, "weakref-finalizer")
atexit._clear()
atexit.register(print, "user-callback")
print("before-exit")
