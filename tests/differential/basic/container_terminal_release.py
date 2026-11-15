"""Terminal release of every container kind, against CPython.

A container's last owner releases its items in CPython's order: a list or
tuple last to first, a dict or set in insertion order. Covers empty, scalar,
heap, mixed and nested payloads; subclasses; weak references to the container
and to its items; finalizers that read other state; resurrection; cycles the
collector frees; and items stored by `xs[i] = v` after construction.
"""

import gc
import weakref

events = []


class Item:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append(self.name)


class Probe:
    """A finalizer that reads the list it was stored next to."""

    def __init__(self, name, other):
        self.name = name
        self.other = other

    def __del__(self):
        events.append((self.name, len(self.other)))


def drop(label, make):
    events.clear()
    value = make()
    del value
    print(label, events)


# Every container kind, scalar and heap payloads.
drop("list empty", lambda: [])
drop("list ints", lambda: [1, 2, 3, 2**40, -7])
drop("list bigints", lambda: [2**70, -(2**80), 3])
drop("list floats", lambda: [1.5, float("inf"), -0.0])
drop("list mixed", lambda: [1, Item("a"), None, Item("b"), True, Item("c")])
drop("list strings", lambda: ["x" * n for n in range(5)])
drop("tuple empty", lambda: ())
drop("tuple ints", lambda: (1, 2, 3))
drop("tuple heap", lambda: (Item("t1"), Item("t2"), Item("t3")))
drop("tuple mixed", lambda: (0, Item("m1"), 2.5, Item("m2")))
drop("tuple pair", lambda: (Item("p1"), Item("p2")))
drop("dict", lambda: {"k1": Item("v1"), Item("k2"): 2, 3: Item("v3")})
events.clear()
members = {Item("s1"), Item("s2")}
del members
print("set", sorted(events))
drop("frozenset", lambda: frozenset([1, 2, 3]))
drop("slice", lambda: slice(Item("start"), Item("stop"), None))
drop("range", lambda: range(2**70, 2**71, 2**60))
drop("nested", lambda: [[Item("n1"), (Item("n2"), [Item("n3")])], Item("n4")])


class ListSub(list):
    pass


class TupleSub(tuple):
    pass


drop("list subclass", lambda: ListSub([Item("ls1"), 4, Item("ls2")]))
drop("tuple subclass", lambda: TupleSub((Item("ts1"), Item("ts2"))))

# Items stored after construction, by index store and by append.
events.clear()
xs = [None] * 4
xs[1] = Item("store1")
xs[3] = Item("store3")
xs.append(Item("append"))
print("popped", xs.pop(1).name, events)
xs[0] = Item("store0")
xs[0] = 7
print("overwritten", events)
del xs
print("index stores", events)

events.clear()
ys = list(range(6))
ys[2] = Item("int-list store")
ys[4] = "s" * 3
del ys
print("int list store", events)

# A finalizer that reads a container still alive beside the dying one.
events.clear()
keep = [1, 2, 3]
dying = [Probe("probe1", keep), Probe("probe2", keep)]
del dying
print("probe", events)

# Weak references to the container and to its items.
events.clear()
holder = [Item("w1"), Item("w2")]
item_ref = weakref.ref(holder[0], lambda r: events.append("item callback"))


class WeakList(list):
    pass


wl = WeakList([Item("wl1"), 9])
list_ref = weakref.ref(wl, lambda r: events.append("list callback"))
del holder
del wl
print("weakrefs", events, item_ref() is None, list_ref() is None)

# A finalizer that resurrects an item keeps it alive past the container.
saved = []


class Resurrect:
    def __del__(self):
        events.append("resurrect")
        saved.append(self)


events.clear()
drop("resurrect", lambda: [Resurrect(), Item("after")])
print("saved", len(saved), events)
saved.clear()

# Cycles through lists, tuples and dicts, freed by the collector.
gc.collect()
events.clear()
a = [Item("cyc-a")]
a.append(a)
b = (Item("cyc-b"), a)
d = {"self": None, "item": Item("cyc-d")}
d["self"] = d
del a, b, d
gc.collect()
print("cycles", sorted(events))

# A bound method owns its receiver; a call's argument builder owns its
# arguments until the call consumes them, and a failed call releases them in
# CPython's value-stack order.
class Owner(Item):
    def method(self):
        return self.name


def takes_two(a, b):
    return 0


def calls():
    events.clear()
    bound = Owner("bm-receiver").method
    print("bound alive", events)
    del bound
    print("bound method", events)
    events.clear()
    print("call result", Owner("tmp").method(), events)
    events.clear()
    try:
        takes_two(Item("arg1"), Item("arg2"), Item("arg3"))
    except TypeError:
        pass
    print("failed call", events)
    events.clear()
    try:
        takes_two(Item("p1"), c=Item("kw"))
    except TypeError:
        pass
    print("failed keyword call", events)


calls()

# Many frees of short tuples and lists in a loop.
events.clear()
total = 0
for i in range(2000):
    t = (i, i + 1)
    lst = [i, str(i), (i,)]
    total += t[0] + len(lst[1])
print("loop", total, events)

# A deep chain releases without overflowing the stack.
chain = None
for i in range(100000):
    chain = [chain, i]
del chain
chain = None
for i in range(100000):
    chain = (chain, i)
del chain
print("deep chains released")
