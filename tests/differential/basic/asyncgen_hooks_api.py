"""Purpose: differential coverage for sys asyncgen hooks API."""

import sys


hooks = sys.get_asyncgen_hooks()
print(
    "orig",
    hooks.firstiter is None,
    hooks.finalizer is None,
    type(hooks).__name__,
)


def first(agen):
    return None


def final(agen):
    return None


ret = sys.set_asyncgen_hooks(firstiter=first, finalizer=final)
print("ret", ret)

cur = sys.get_asyncgen_hooks()
print("cur", cur.firstiter is first, cur.finalizer is final)

try:
    sys.set_asyncgen_hooks(firstiter=1, finalizer=final)
except Exception as exc:
    print("bad_first", type(exc).__name__, str(exc))

try:
    sys.set_asyncgen_hooks(firstiter=first, finalizer=1)
except Exception as exc:
    print("bad_final", type(exc).__name__, str(exc))

sys.set_asyncgen_hooks(firstiter=hooks.firstiter, finalizer=hooks.finalizer)
