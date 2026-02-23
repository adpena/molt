"""Purpose: capture shutdown order when finalize is registered before atexit."""

import atexit
import weakref


class Box:
    pass


live = Box()
weakref.finalize(live, print, "weakref-finalizer")
atexit.register(print, "user-callback")
print("before-exit")
