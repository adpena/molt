"""List slicing, slice assignment and terminal release, against CPython.

Covers the runtime paths that copy only the selected items, overwrite an
equal-length range in place and retire a payload of non-heap words without the
edge sink, with the cases where each must decline: heap items, big-int bounds,
negative and extreme steps, aliasing, subclasses, weak references and
finalizers.
"""

import gc
import weakref


def show(label, value):
    print(label, value)


# Slicing with every bound form: None, negative, out of range, big ints.
base = list(range(10))
big = 10**30
for bounds in [
    (None, None, None),
    (2, 7, None),
    (-3, None, None),
    (None, -3, None),
    (7, 2, -1),
    (None, None, -1),
    (9, None, -2),
    (-1, -11, -3),
    (big, -big, -1),
    (-big, big, 2),
    (big, None, None),
    (None, -big, None),
    (3, 3, None),
    (5, 4, None),
    (0, 1, None),
    (True, 4, True),
    (None, None, 2**62),
    (None, None, -(2**62)),
]:
    s = slice(*bounds)
    show("get", (bounds, base[s]))

show("empty", ([][::-1], [][1:], [7][::-1], [7][0:1], [7][1:]))

# Items that own references are copied with their references.
objs = [object() for _ in range(4)]
copy = objs[::-1]
show("ref copy", [a is b for a, b in zip(copy, reversed(objs))])
del objs
show("ref copy alive", len(copy))

# Equal-length replacement: in place for non-heap words, general otherwise.
a = list(range(8))
a[2:5] = [20, 30, 40]
show("eq len", a)
a[2:5] = ["x", "y", "z"]
show("eq len heap", a)
a[2:5] = [1, 2, 3]
show("eq len heap removed", a)
a[1:3] = []
show("shrink", a)
a[1:1] = [9, 9]
show("grow", a)

# Aliasing: the source is the target.
a = list(range(6))
a[:] = a[::-1]
show("alias full", a)
a = list(range(6))
a[1:4] = a[3:0:-1]
show("alias mid", a)
a = list(range(6))
a[:3] = a
show("alias self grow", a)


# The prefix reverse idiom, every case: in range, past the end, zero,
# negative (which grows the list), a bool and a heap int index.
def prefix_reverse(seq, k):
    seq[: k + 1] = seq[k::-1]
    return seq


for k in [0, 1, 3, 5, 9, 50, -1, -3, True, False, 2**70, -(2**70)]:
    show("prefix", (k, prefix_reverse(list(range(6)), k)))
show("prefix empty", prefix_reverse([], 2))
show("prefix one", prefix_reverse([4], 0))
show("prefix refs", prefix_reverse(["a", "b", "c", "d"], 2))


class Recorded(list):
    def __getitem__(self, key):
        print("getitem", key)
        return list.__getitem__(self, key)

    def __setitem__(self, key, value):
        print("setitem", key, value)
        list.__setitem__(self, key, value)


show("prefix subclass", prefix_reverse(Recorded(range(5)), 2))


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        print("index", self.value)
        return self.value

    def __add__(self, other):
        print("add", self.value, other)
        return Index(self.value + other)


show("prefix index", prefix_reverse(list(range(6)), Index(3)))
try:
    prefix_reverse((1, 2, 3), 1)
except TypeError as exc:
    show("prefix tuple", exc)
try:
    prefix_reverse(list(range(4)), "2")
except TypeError as exc:
    show("prefix str", exc)


# A loop of flips, as fannkuch runs it.
def flips(perm):
    perm = perm[:]
    count = 0
    k = perm[0]
    while k:
        perm[: k + 1] = perm[k::-1]
        count += 1
        k = perm[0]
    return count, perm


show("flips", flips([3, 1, 4, 0, 2]))
show("flips 2", flips([2, 0, 1]))

# Mutation during iteration sees the live list.
a = list(range(5))
seen = []
for x in a:
    seen.append(x)
    if x == 1:
        a[2:4] = a[3:1:-1]
show("mutate iter", (seen, a))


# Terminal release: subclasses, weak references, finalizers, nested items.
class Tracked(list):
    def __del__(self):
        print("del Tracked", list(self))


t = Tracked([1, 2, 3])
del t
gc.collect()


class Plain(list):
    pass


p = Plain([1, 2])
r = weakref.ref(p)
show("weak alive", r() is p)
del p
gc.collect()
show("weak dead", r() is None)


class Noisy:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        print("del", self.name)


holder = [1, 2, Noisy("in list")]
del holder
holder = (1, Noisy("in tuple"))
del holder
holder = [[1, 2], [Noisy("nested")]]
del holder
s = slice(1, Noisy("in slice"))
del s
gc.collect()
show("sizes", (len([0] * 1000), len(list(range(3))[::-1]), (1, 2)[::-1]))
