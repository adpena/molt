"""Purpose: differential coverage for weakref.finalize detach/peek edges."""

import gc
import weakref


class Box:
    pass


events = []


def callback(tag):
    events.append(tag)
    return f"rv:{tag}"


print("case-call-start")
obj = Box()
fin = weakref.finalize(obj, callback, "manual")
peek = fin.peek()
print(
    "call-peek",
    isinstance(peek, tuple),
    len(peek) if peek else None,
    peek[0] is obj if peek else None,
    peek[2] if peek else None,
)
print("call-alive0", fin.alive)
print("call-ret1", fin())
print("call-ret2", fin())
print("call-alive1", fin.alive)
print("call-peek1", fin.peek())
print("call-detach1", fin.detach())
print("call-events", events)

print("case-detach-start")
obj = Box()
fin = weakref.finalize(obj, callback, "detached")
peek = fin.peek()
print(
    "detach-peek",
    isinstance(peek, tuple),
    len(peek) if peek else None,
    peek[0] is obj if peek else None,
    peek[2] if peek else None,
)
detached = fin.detach()
print(
    "detach-ret",
    isinstance(detached, tuple),
    len(detached) if detached else None,
    detached[0] is obj if detached else None,
    detached[2] if detached else None,
)
print("detach-alive1", fin.alive)
print("detach-call1", fin())
print("detach-peek1", fin.peek())
print("detach-detach1", fin.detach())
if detached is not None:
    _, fn, args, kwargs = detached
    print("detach-manual-call", fn(*args, **kwargs))
print("detach-events", events)

print("case-gc-start")
obj = Box()
fin = weakref.finalize(obj, callback, "gc")
print("gc-alive0", fin.alive)
obj = None
gc.collect()
print("gc-alive1", fin.alive)
print("gc-call1", fin())
print("gc-peek1", fin.peek())
print("gc-detach1", fin.detach())
print("gc-events", events)
