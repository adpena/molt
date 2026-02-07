"""Purpose: differential coverage for disabling weakref.finalize atexit callbacks."""

import weakref


class Box:
    pass


live = Box()
fin = weakref.finalize(live, print, "should-not-print")
fin.atexit = False
print("alive-before", fin.alive)
print("atexit-flag", fin.atexit)
print("before-exit")
