"""Purpose: differential coverage for weakref.finalize atexit behavior."""

import weakref


class Box:
    pass


live = Box()
fin = weakref.finalize(live, print, "atexit-finalizer-fired")
print("alive-before", fin.alive)
print("peek-before", fin.peek() is not None)
print("before-exit")
