"""Purpose: differential coverage for weakref lifetime vs gc.collect."""

import gc
import weakref


class Token:
    pass


def make_ref():
    obj = Token()
    ref = weakref.ref(obj)
    print("alive", ref() is not None)
    return ref


ref = make_ref()
print("after-drop", ref() is not None)
gc.collect()
print("after-collect", ref() is not None)
