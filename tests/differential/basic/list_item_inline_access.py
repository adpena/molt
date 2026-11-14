"""`xs[i]` and `xs[i] = v` on a list keep CPython's meaning on every edge.

CPython oracle: Objects/listobject.c list_subscript and list_ass_subscript
adjust a negative index by the length, raise IndexError("list index out of
range") / IndexError("list assignment index out of range") outside it, run
__index__ for a non-int index, and dispatch a subclass's own
__getitem__/__setitem__. list_ass_item stores the new item before it drops
the old one. Molt compiles an exact list at an inline int index inline
(molt_codegen_abi::list_item) and sends every other case to the runtime;
this program crosses that boundary in both directions on every storage kind.
The differential runner supplies matching CPython target expectations.
"""


def read(xs, i):
    try:
        return xs[i]
    except IndexError as error:
        return "IndexError: " + str(error)
    except TypeError as error:
        return "TypeError: " + str(error)


def write(xs, i, v):
    try:
        xs[i] = v
        return "ok"
    except IndexError as error:
        return "IndexError: " + str(error)
    except TypeError as error:
        return "TypeError: " + str(error)


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class Logged(list):
    def __getitem__(self, i):
        return ("sub-get", i, list.__getitem__(self, i))

    def __setitem__(self, i, v):
        list.__setitem__(self, i, ("sub-set", v))


words = ["a", "b", "c"]
ints = [10, 20, 30]
bools = [True, False, True]
empty = []
for xs in (words, ints, bools, empty):
    print([read(xs, i) for i in (-4, -3, -1, 0, 1, 2, 3)])
    print(read(xs, True), read(xs, False))
    print(read(xs, 2**80), read(xs, -(2**80)))
    print(read(xs, Index(-1)), read(xs, Index(5)))
    print(read(xs, 1.0), read(xs, "1"))

for xs in (list(words), list(ints), list(bools), []):
    print(write(xs, -1, "z"), write(xs, 3, "z"), write(xs, -4, "z"), xs)
    print(write(xs, True, 7), write(xs, Index(0), 8), write(xs, 2**80, 9), xs)

# Compact lists keep or leave their storage kind as values change.
ints = [1, 2, 3]
ints[0] = -5
ints[1] = "s"
ints[2] = 2**70
print(ints, [type(x).__name__ for x in ints])
bools = [False, False]
bools[0] = True
bools[1] = 1
print(bools, [type(x).__name__ for x in bools])

sub = Logged([1, 2, 3])
print(sub[0], sub[-1])
sub[1] = 9
print(list.__getitem__(sub, 1), sub[1])

# Iterating by index while the loop body mutates the list.
xs = list(range(6))
i = 0
while i < len(xs):
    if xs[i] % 2 == 0:
        xs[i] = xs[i] * 10
    if i == 2:
        xs.append(99)
    i += 1
print(xs)


class Witness:
    log = []

    def __init__(self, tag, target):
        self.tag = tag
        self.target = target

    def __del__(self):
        # The store already holds the new item when the old one dies.
        Witness.log.append((self.tag, read(self.target, 0)))


holder = [None]
holder[0] = Witness("first", holder)
holder[0] = "replacement"
print(Witness.log)

# The last reference to the old item is the slot itself.
holder = [[1, 2, 3]]
holder[0] = holder[0][1]
print(holder)
