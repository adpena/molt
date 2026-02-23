"""Purpose: weakref.finalize should precede older atexit callbacks."""

import atexit
import weakref


class Box:
    pass


atexit.register(print, "user-callback")
live = Box()
weakref.finalize(live, print, "weakref-finalizer")
print("before-exit")
