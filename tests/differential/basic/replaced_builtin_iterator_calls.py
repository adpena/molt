"""Replacing each builtin among the iterator call lowerings reaches every read
of it (HF-776).

The frontend lowers a proved read of these builtins by name: `slice`, `aiter`,
`anext`, `any`, `all`, `sum`, `map`, `zip`, `min`, `max`, `iter`, `next`,
`enumerate`, `filter`, `reversed`, `sorted`.
For each one in turn, a module binding, a `builtins` member write and a
`__builtins__` mapping select another object, so every read below must reach
the replacement, as in CPython. The mapping form comes last: a function takes
its builtins from the mapping when it is created.
"""

import builtins

KINDS = (
    "Async",
    "enumerate",
    "filter",
    "list_iterator",
    "list_reverseiterator",
    "map",
    "zip",
)
NAMES = (
    "slice",
    "aiter",
    "anext",
    "any",
    "all",
    "sum",
    "map",
    "zip",
    "min",
    "max",
    "iter",
    "next",
    "enumerate",
    "filter",
    "reversed",
    "sorted",
)
ORIGINALS = {name: getattr(builtins, name) for name in NAMES}


class Fake:
    """A replacement whose value and call results print deterministically."""

    def __init__(self, name):
        self.name = name

    def __call__(self, *args, **kwargs):
        return "called-" + self.name

    def __repr__(self):
        return "fake-" + self.name


def show(value):
    # Reads no replaceable builtin: the program replaces them.
    return value.__class__.__name__ if value.__class__.__name__ in KINDS else value


class Async:
    def __aiter__(self):
        return self

    def __anext__(self):
        return "anext-result"


ITERATOR = iter([4, 5, 6, 7, 8, 9])


def use_slice():
    return slice(1, 2)


print("slice original", show(use_slice()), show(slice(1, 2)))
slice = Fake("slice-module")
print("slice module binding", show(use_slice()), show(slice(1, 2)))
del slice
print("slice module binding deleted", show(use_slice()), show(slice(1, 2)))
builtins.slice = Fake("slice-member")
print("slice member", show(use_slice()), show(slice(1, 2)))
builtins.slice = ORIGINALS["slice"]
print("slice member restored", show(use_slice()), show(slice(1, 2)))


def use_aiter():
    return aiter(Async())


print("aiter original", show(use_aiter()), show(aiter(Async())))
aiter = Fake("aiter-module")
print("aiter module binding", show(use_aiter()), show(aiter(Async())))
del aiter
print("aiter module binding deleted", show(use_aiter()), show(aiter(Async())))
builtins.aiter = Fake("aiter-member")
print("aiter member", show(use_aiter()), show(aiter(Async())))
builtins.aiter = ORIGINALS["aiter"]
print("aiter member restored", show(use_aiter()), show(aiter(Async())))


def use_anext():
    return anext(Async())


print("anext original", show(use_anext()), show(anext(Async())))
anext = Fake("anext-module")
print("anext module binding", show(use_anext()), show(anext(Async())))
del anext
print("anext module binding deleted", show(use_anext()), show(anext(Async())))
builtins.anext = Fake("anext-member")
print("anext member", show(use_anext()), show(anext(Async())))
builtins.anext = ORIGINALS["anext"]
print("anext member restored", show(use_anext()), show(anext(Async())))


def use_any():
    return any([0, 1])


print("any original", show(use_any()), show(any([0, 1])))
any = Fake("any-module")
print("any module binding", show(use_any()), show(any([0, 1])))
del any
print("any module binding deleted", show(use_any()), show(any([0, 1])))
builtins.any = Fake("any-member")
print("any member", show(use_any()), show(any([0, 1])))
builtins.any = ORIGINALS["any"]
print("any member restored", show(use_any()), show(any([0, 1])))


def use_all():
    return all([1, 0])


print("all original", show(use_all()), show(all([1, 0])))
all = Fake("all-module")
print("all module binding", show(use_all()), show(all([1, 0])))
del all
print("all module binding deleted", show(use_all()), show(all([1, 0])))
builtins.all = Fake("all-member")
print("all member", show(use_all()), show(all([1, 0])))
builtins.all = ORIGINALS["all"]
print("all member restored", show(use_all()), show(all([1, 0])))


def use_sum():
    return sum([1, 2])


print("sum original", show(use_sum()), show(sum([1, 2])))
sum = Fake("sum-module")
print("sum module binding", show(use_sum()), show(sum([1, 2])))
del sum
print("sum module binding deleted", show(use_sum()), show(sum([1, 2])))
builtins.sum = Fake("sum-member")
print("sum member", show(use_sum()), show(sum([1, 2])))
builtins.sum = ORIGINALS["sum"]
print("sum member restored", show(use_sum()), show(sum([1, 2])))


def use_map():
    return map(str, [1])


print("map original", show(use_map()), show(map(str, [1])))
map = Fake("map-module")
print("map module binding", show(use_map()), show(map(str, [1])))
del map
print("map module binding deleted", show(use_map()), show(map(str, [1])))
builtins.map = Fake("map-member")
print("map member", show(use_map()), show(map(str, [1])))
builtins.map = ORIGINALS["map"]
print("map member restored", show(use_map()), show(map(str, [1])))


def use_zip():
    return zip([1], [2])


print("zip original", show(use_zip()), show(zip([1], [2])))
zip = Fake("zip-module")
print("zip module binding", show(use_zip()), show(zip([1], [2])))
del zip
print("zip module binding deleted", show(use_zip()), show(zip([1], [2])))
builtins.zip = Fake("zip-member")
print("zip member", show(use_zip()), show(zip([1], [2])))
builtins.zip = ORIGINALS["zip"]
print("zip member restored", show(use_zip()), show(zip([1], [2])))


def use_min():
    return min(3, 9)


print("min original", show(use_min()), show(min(3, 9)))
min = Fake("min-module")
print("min module binding", show(use_min()), show(min(3, 9)))
del min
print("min module binding deleted", show(use_min()), show(min(3, 9)))
builtins.min = Fake("min-member")
print("min member", show(use_min()), show(min(3, 9)))
builtins.min = ORIGINALS["min"]
print("min member restored", show(use_min()), show(min(3, 9)))


def use_max():
    return max(3, 9)


print("max original", show(use_max()), show(max(3, 9)))
max = Fake("max-module")
print("max module binding", show(use_max()), show(max(3, 9)))
del max
print("max module binding deleted", show(use_max()), show(max(3, 9)))
builtins.max = Fake("max-member")
print("max member", show(use_max()), show(max(3, 9)))
builtins.max = ORIGINALS["max"]
print("max member restored", show(use_max()), show(max(3, 9)))


def use_iter():
    return iter([7])


print("iter original", show(use_iter()), show(iter([7])))
iter = Fake("iter-module")
print("iter module binding", show(use_iter()), show(iter([7])))
del iter
print("iter module binding deleted", show(use_iter()), show(iter([7])))
builtins.iter = Fake("iter-member")
print("iter member", show(use_iter()), show(iter([7])))
builtins.iter = ORIGINALS["iter"]
print("iter member restored", show(use_iter()), show(iter([7])))


def use_next():
    return next(ITERATOR, 'done')


print("next original", show(use_next()), show(next(ITERATOR, 'done')))
next = Fake("next-module")
print("next module binding", show(use_next()), show(next(ITERATOR, 'done')))
del next
print("next module binding deleted", show(use_next()), show(next(ITERATOR, 'done')))
builtins.next = Fake("next-member")
print("next member", show(use_next()), show(next(ITERATOR, 'done')))
builtins.next = ORIGINALS["next"]
print("next member restored", show(use_next()), show(next(ITERATOR, 'done')))


def use_enumerate():
    return enumerate(['a'])


print("enumerate original", show(use_enumerate()), show(enumerate(['a'])))
enumerate = Fake("enumerate-module")
print("enumerate module binding", show(use_enumerate()), show(enumerate(['a'])))
del enumerate
print("enumerate module binding deleted", show(use_enumerate()), show(enumerate(['a'])))
builtins.enumerate = Fake("enumerate-member")
print("enumerate member", show(use_enumerate()), show(enumerate(['a'])))
builtins.enumerate = ORIGINALS["enumerate"]
print("enumerate member restored", show(use_enumerate()), show(enumerate(['a'])))


def use_filter():
    return filter(None, [0, 1])


print("filter original", show(use_filter()), show(filter(None, [0, 1])))
filter = Fake("filter-module")
print("filter module binding", show(use_filter()), show(filter(None, [0, 1])))
del filter
print("filter module binding deleted", show(use_filter()), show(filter(None, [0, 1])))
builtins.filter = Fake("filter-member")
print("filter member", show(use_filter()), show(filter(None, [0, 1])))
builtins.filter = ORIGINALS["filter"]
print("filter member restored", show(use_filter()), show(filter(None, [0, 1])))


def use_reversed():
    return reversed([1, 2])


print("reversed original", show(use_reversed()), show(reversed([1, 2])))
reversed = Fake("reversed-module")
print("reversed module binding", show(use_reversed()), show(reversed([1, 2])))
del reversed
print("reversed module binding deleted", show(use_reversed()), show(reversed([1, 2])))
builtins.reversed = Fake("reversed-member")
print("reversed member", show(use_reversed()), show(reversed([1, 2])))
builtins.reversed = ORIGINALS["reversed"]
print("reversed member restored", show(use_reversed()), show(reversed([1, 2])))


def use_sorted():
    return sorted([3, 1])


print("sorted original", show(use_sorted()), show(sorted([3, 1])))
sorted = Fake("sorted-module")
print("sorted module binding", show(use_sorted()), show(sorted([3, 1])))
del sorted
print("sorted module binding deleted", show(use_sorted()), show(sorted([3, 1])))
builtins.sorted = Fake("sorted-member")
print("sorted member", show(use_sorted()), show(sorted([3, 1])))
builtins.sorted = ORIGINALS["sorted"]
print("sorted member restored", show(use_sorted()), show(sorted([3, 1])))

custom = dict(builtins.__dict__)
custom["slice"] = Fake("slice-mapping")
custom["aiter"] = Fake("aiter-mapping")
custom["anext"] = Fake("anext-mapping")
custom["any"] = Fake("any-mapping")
custom["all"] = Fake("all-mapping")
custom["sum"] = Fake("sum-mapping")
custom["map"] = Fake("map-mapping")
custom["zip"] = Fake("zip-mapping")
custom["min"] = Fake("min-mapping")
custom["max"] = Fake("max-mapping")
custom["iter"] = Fake("iter-mapping")
custom["next"] = Fake("next-mapping")
custom["enumerate"] = Fake("enumerate-mapping")
custom["filter"] = Fake("filter-mapping")
custom["reversed"] = Fake("reversed-mapping")
custom["sorted"] = Fake("sorted-mapping")
__builtins__ = custom


def after_slice():
    return slice(1, 2)


print("slice mapping", show(use_slice()), show(after_slice()))


def after_aiter():
    return aiter(Async())


print("aiter mapping", show(use_aiter()), show(after_aiter()))


def after_anext():
    return anext(Async())


print("anext mapping", show(use_anext()), show(after_anext()))


def after_any():
    return any([0, 1])


print("any mapping", show(use_any()), show(after_any()))


def after_all():
    return all([1, 0])


print("all mapping", show(use_all()), show(after_all()))


def after_sum():
    return sum([1, 2])


print("sum mapping", show(use_sum()), show(after_sum()))


def after_map():
    return map(str, [1])


print("map mapping", show(use_map()), show(after_map()))


def after_zip():
    return zip([1], [2])


print("zip mapping", show(use_zip()), show(after_zip()))


def after_min():
    return min(3, 9)


print("min mapping", show(use_min()), show(after_min()))


def after_max():
    return max(3, 9)


print("max mapping", show(use_max()), show(after_max()))


def after_iter():
    return iter([7])


print("iter mapping", show(use_iter()), show(after_iter()))


def after_next():
    return next(ITERATOR, 'done')


print("next mapping", show(use_next()), show(after_next()))


def after_enumerate():
    return enumerate(['a'])


print("enumerate mapping", show(use_enumerate()), show(after_enumerate()))


def after_filter():
    return filter(None, [0, 1])


print("filter mapping", show(use_filter()), show(after_filter()))


def after_reversed():
    return reversed([1, 2])


print("reversed mapping", show(use_reversed()), show(after_reversed()))


def after_sorted():
    return sorted([3, 1])


print("sorted mapping", show(use_sorted()), show(after_sorted()))
