"""Two-target loops step and unpack in one call when the item is unobservable
(TIR pass pair_unpack); every other shape keeps its tuple. Values, errors,
tracebacks and finalizer order must stay CPython's."""


def items_loop(d):
    out = []
    for k, v in d.items():
        out.append((k, v))
    return out


def comprehension(d):
    return {v: k for k, v in d.items()}


def reversed_items(d):
    return [k * 10 + v for k, v in reversed(d.items())]


def enumerate_loop(xs, start=0):
    return [(i, x) for i, x in enumerate(xs, start)]


def zip_loop(a, b, strict=False):
    return [x + y for x, y in zip(a, b, strict=strict)]


def held_pair(d):
    kept = []
    for pair in d.items():
        k, v = pair
        kept.append(pair)
    return kept, [type(p).__name__ for p in kept]


def three_targets(rows):
    return [a + b + c for a, b, c in rows]


def generic_items(rows):
    return [(a, b) for a, b in iter(rows)]


def report(label, fn, *args):
    try:
        print(label, fn(*args))
    except Exception as exc:  # noqa: BLE001 - the error is the result
        tb = exc.__traceback__
        while tb.tb_next is not None:
            tb = tb.tb_next
        print(label, type(exc).__name__, exc, tb.tb_lineno - tb.tb_frame.f_code.co_firstlineno)


d = {"a": 1, "b": 2, "c": 3}
report("items", items_loop, d)
report("comprehension", comprehension, d)
report("reversed", reversed_items, {1: 2, 3: 4})
report("enumerate", enumerate_loop, ["x", "y"])
report("enumerate_bigint", enumerate_loop, ["x", "y", "z"], 2**63 - 2)
report("zip", zip_loop, [1, 2, 3], [10, 20, 30])
report("zip_short", zip_loop, [1, 2, 3], [10])
report("zip_strict", zip_loop, [1, 2], [10], True)
report("held", held_pair, d)
report("three", three_targets, [(1, 2, 3), (4, 5, 6)])
report("generic", generic_items, [(1, 2), [3, 4], "ab", range(5, 7)])

# Unpack errors: wrong length both ways, a non-iterable item, an iterator item.
report("too_many", generic_items, [(1, 2), (1, 2, 3)])
report("too_few", generic_items, [(1,)])
report("not_iterable", generic_items, [(1, 2), 5])


def gen_pair(a, b):
    yield a
    yield b


report("generator_item", generic_items, [gen_pair(1, 2), gen_pair(3, 4)])
report("generator_short", generic_items, [gen_pair(1, 2), iter([7])])
report("zip3", lambda: [(a, b) for a, b in zip([1], [2], [3])])


def own_line_target(rows):
    out = []
    for (
        a,
        b,
    ) in rows:
        out.append(a + b)
    return out


report("own_line_ok", own_line_target, [(1, 2)])
report("own_line_error", own_line_target, [(1, 2, 3)])


def in_try(rows):
    out = []
    try:
        for a, b in rows:
            out.append(a * b)
    except ValueError as exc:
        out.append(str(exc))
    return out


report("in_try", in_try, [(2, 3), (4, 5)])
report("in_try_error", in_try, [(2, 3), (4, 5, 6)])


def grow_while_iterating(d):
    for k, v in d.items():
        d[k + "!"] = v


def rewrite_same_size(d):
    seen = []
    for k, v in d.items():
        seen.append((k, v))
        d[k] = v * 100
    return seen, sorted(d.items())


report("grow", grow_while_iterating, {"a": 1})
report("same_size", rewrite_same_size, {"a": 1, "b": 2})


class Mutating:
    """A key whose __eq__ grows the dict being iterated, once armed."""

    armed = False

    def __init__(self, name, target):
        self.name = name
        self.target = target

    def __hash__(self):
        return 7

    def __eq__(self, other):
        if Mutating.armed:
            Mutating.armed = False
            self.target["grown"] = 0
        return self is other

    def __repr__(self):
        return self.name


def reentrant_eq():
    d = {}
    a = Mutating("a", d)
    b = Mutating("b", d)
    d[a] = 1
    d[b] = 2
    out = []
    probe = {a: "x"}
    for k, v in d.items():
        out.append((repr(k), v))
        Mutating.armed = True
        probe.get(b)  # a.__eq__(b) runs in the body and grows d
    return out


report("reentrant_eq", reentrant_eq)

events = []


class Noisy:
    """An iterable item whose release runs Python code."""

    def __init__(self, name):
        self.name = name

    def __iter__(self):
        return iter((self.name, len(self.name)))

    def __del__(self):
        events.append("del " + self.name)


def finalizer_order():
    for name, size in (Noisy(n) for n in ["ab", "cde"]):
        events.append("body " + name + str(size))
    return events


report("finalizer_order", finalizer_order)


class ItemsOverride(dict):
    def items(self):
        return [("over", "ridden")]


report("subclass_items", items_loop, ItemsOverride(a=1))


def release_after_loop():
    log = []

    class Tracked:
        def __init__(self, n):
            self.n = n

        def __del__(self):
            log.append(self.n)

    d = {i: Tracked(i) for i in range(3)}
    for k, v in d.items():
        log.append(k)
    d.clear()
    log.append("cleared")
    del v
    log.append("deleted")
    return log


report("release_after_loop", release_after_loop)


# CPython's dict items, enumerate and zip iterators keep their last pair alive
# until the next step: a target deleted in the body is finalized only then.
lifetimes = []


class Lifetime:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        lifetimes.append(("del", self.n))


def items_keep_last_pair():
    d = {1: Lifetime(1), 2: Lifetime(2)}
    for k, v in d.items():
        if k == 1:
            d[1] = None
            del v
            lifetimes.append("after del")
    lifetimes.append("loop done")


def enumerate_keeps_last_pair():
    xs = [Lifetime(10), Lifetime(11)]
    for i, x in enumerate(xs):
        if i == 0:
            xs[0] = None
            del x
            lifetimes.append("after del")
    lifetimes.append("loop done")


def zip_keeps_last_pair():
    xs = [Lifetime(20), Lifetime(21)]
    for x, y in zip(xs, [0, 1]):
        if y == 0:
            xs[0] = None
            del x
            lifetimes.append("after del")
    lifetimes.append("loop done")


for keep in (items_keep_last_pair, enumerate_keeps_last_pair, zip_keeps_last_pair):
    keep()
    print(keep.__name__, lifetimes)
    lifetimes.clear()
