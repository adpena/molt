"""Purpose: differential coverage for weakref basic behavior."""

import gc
import weakref


class Thing:
    pass


if __name__ == "__main__":
    obj = Thing()
    ref = weakref.ref(obj)
    print("alive", ref() is obj)
    del obj
    gc.collect()
    print("cleared", ref() is None)
