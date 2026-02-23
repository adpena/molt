"""Purpose: weakref.finalize shutdown callbacks survive atexit.unregister rewrites."""

import atexit
import weakref


class Box:
    pass


live = Box()
weakref.finalize(live, print, "weakref-finalizer")
atexit.register(print, "user-callback")
atexit.unregister(print)
print("before-exit")
