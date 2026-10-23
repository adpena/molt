"""Purpose: differential coverage for the native iterator step (tp_iternext).

Builtin and itertools iterators advance through one native step. These cases
pin the observable protocol around it: StopIteration values, error
propagation, zip's advance order, reused result tuples, and itertools types
that are immutable, so their native `__next__` cannot be rebound.
"""

import itertools


def returns(value):
    yield 1
    return value


def delegate(wrap):
    result = yield from wrap(returns("ret"))
    print("result", repr(result))


for name, wrap in (
    ("enumerate", enumerate),
    ("map", lambda it: map(lambda v: v, it)),
    ("filter", lambda it: filter(None, it)),
    ("zip", lambda it: zip(it)),
    ("zip-first", lambda it: zip(it, "ab")),
    ("zip-strict", lambda it: zip(it, strict=True)),
    ("reversed", lambda it: reversed(list(it))),
    ("chain", lambda it: itertools.chain(it)),
    ("tee", lambda it: itertools.tee(it)[0]),
):
    print(name, list(delegate(wrap)))

stopped = returns(5)
next(stopped)
try:
    next(stopped)
except StopIteration as exc:
    print("stop value", exc.args, exc.value)
try:
    next(iter([]))
except StopIteration as exc:
    print("stop empty", exc.args, exc.value)
print(next(iter(()), "default"), next(returns(0), "unused"))
try:
    next(5)
except TypeError as exc:
    print("TypeError", exc)


class NoBool:
    def __bool__(self):
        raise ValueError("no bool")


for predicate in (None, lambda item: item):
    try:
        list(filter(predicate, [NoBool()]))
    except ValueError as exc:
        print("ValueError", exc)


class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise KeyError("boom")


for name, wrap in (
    ("enumerate", enumerate),
    ("zip", lambda it: zip(it)),
    ("map", lambda it: map(str, it)),
    ("filter", lambda it: filter(None, it)),
    ("chain", lambda it: itertools.chain(it)),
    ("tee", lambda it: itertools.tee(it)[0]),
):
    try:
        list(wrap(Boom()))
    except KeyError as exc:
        print(name, "KeyError", exc)

seen = []


def tracked(name, count):
    for index in range(count):
        seen.append((name, index))
        yield index


print(list(zip(tracked("a", 1), tracked("b", 3))), seen)
seen.clear()
try:
    list(zip(tracked("a", 1), tracked("b", 2), tracked("c", 3), strict=True))
except ValueError as exc:
    print("ValueError", exc)
print(seen)
seen.clear()
try:
    list(zip(tracked("a", 3), tracked("b", 1), tracked("c", 3), strict=True))
except ValueError as exc:
    print("ValueError", exc)
print(seen)

print(list(map(lambda x, y=10: x + y, [1, 2])), list(map(pow, [2, 3], [3, 2])))
print(list(map(lambda a, b, c: a + b + c, [1], [2], [3])))
print(list(map(lambda *parts: sum(parts), [1, 2], [3, 4], [5, 6], [7, 8])))

pairs = zip("ab", "xy")
first = next(pairs)
second = next(pairs)
print(first, second, list(zip(range(3), "abc")))
numbered = enumerate("pq")
first = next(numbered)
second = next(numbered)
print(first, second, [item for item in enumerate("rs", 7)])

for sample in (itertools.count(), itertools.chain([1]), itertools.islice([1], 1),
               itertools.repeat(1)):
    kind = type(sample)
    print(kind.__name__, kind.__module__)
    try:
        kind.__next__ = lambda self: "rebound"
        print("rebound")
    except TypeError:
        print("TypeError")
    print(next(sample))
