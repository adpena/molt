"""Purpose: differential coverage for weakref.ReferenceType.__callback__ parity."""

import gc
import weakref


class Box:
    pass


def _cb(_ref):
    return None


obj = Box()
ref_with_callback = weakref.ref(obj, _cb)
ref_without_callback = weakref.ref(obj)

print("alive-with-callback", ref_with_callback.__callback__ is _cb)
print("alive-without-callback", ref_without_callback.__callback__ is None)

obj = None
gc.collect()

print("dead-with-callback", ref_with_callback.__callback__ is None)
print("dead-without-callback", ref_without_callback.__callback__ is None)
