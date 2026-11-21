"""Purpose: an instance whose fields hold references frees inline (HF-202,
lifecycle S6), dropping each field's reference before the free as CPython's
deallocation does. Each block separates the correct release from a plausible
defect: a child finalizer skipped or run out of order, a shared child freed
while another owner holds it, two fields naming one child released once or
three times, a deep chain overflowing the stack, a weak reference to a child
whose callback must fire, and an instance with a dictionary."""

import gc
import weakref

events = []


class Tracer:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append(("del", self.name))


class Pair:
    def __init__(self, a, b):
        self.a = a
        self.b = b


# Each child's last reference: both finalizers run, before the next statement.
pair = Pair(Tracer("first"), Tracer("second"))
del pair
print("children", sorted(events))
events.clear()

# A shared child survives its holder.
shared = Tracer("shared")
pair = Pair(shared, [1, 2, 3])
del pair
print("shared alive", shared.name, events)

# Two fields naming one child: two references fall, the child lives on
# while the local holds it, then dies once.
pair = Pair(shared, shared)
del pair
print("aliased alive", shared.name, events)
del shared
print("aliased dead", events)
events.clear()

# Two fields holding the child's only references.
pair = Pair(None, None)
child = Tracer("only")
pair.a = child
pair.b = child
del child
del pair
print("aliased only", events)
events.clear()


# A deep chain releases without exhausting the stack.
class Node:
    def __init__(self, nxt):
        self.next = nxt


head = None
for _ in range(200000):
    head = Node(head)
del head
print("chain freed")

# A weak reference to a child: its callback runs when the holder frees it.
target = Tracer("weak")
ref = weakref.ref(target, lambda r: events.append("callback"))
pair = Pair(target, 0)
del target
del pair
print("weak child", events, ref() is None)
events.clear()

# Many holders of one string and one list.
text = "x" * 10
values = [1]
holders = [Pair(text, values) for _ in range(1000)]
del holders
print("holders freed", len(text), values)

# An instance that gained a dictionary entry beyond its fields.
pair = Pair(Tracer("dict-a"), 1)
pair.extra = Tracer("dict-extra")
del pair
print("with extra", sorted(events))
events.clear()

# Cycles through pointer fields stay the collector's.
gc.collect()
left = Pair(None, Tracer("cycle"))
right = Pair(left, None)
left.a = right
del left, right
print("cycle before collect", events)
gc.collect()
print("cycle after collect", events)
