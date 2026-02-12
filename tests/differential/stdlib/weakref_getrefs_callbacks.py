"""Purpose: differential coverage for weakref callback refs and dedupe semantics."""

import gc
import weakref


class Box:
    pass


def _cb(_ref):
    return None


box = Box()
base = weakref.ref(box)
cb1 = weakref.ref(box, _cb)
cb2 = weakref.ref(box, _cb)

refs = weakref.getweakrefs(box)
print("alive-count", weakref.getweakrefcount(box))
print("alive-len", len(refs))
print("alive-has-base", base in refs)
print("alive-has-cb1", cb1 in refs)
print("alive-has-cb2", cb2 in refs)
print("dedupe", weakref.ref(box) is base)

del box
gc.collect()

print("dead-cb-eq", cb1 == cb2)
print("dead-cb-ident", cb1 is cb2)
print("dead-base-self-eq", base == base)
print("nonweakrefable", weakref.getweakrefcount(1), weakref.getweakrefs(1))
