"""A call of a module-level generator function, proven closed-world.

Purpose: a proven call of a generator function allocates its frame directly,
and a `for` loop that is the generator's one consumer runs the generator's
body in place. The observable behavior must equal CPython's in every shape:
fused reductions, break and early return, escapes (`next`, `send`, `throw`,
`close`, attributes), argument evaluation, lifetime, and calls the proof does
not cover (defaults, keywords, rebinding).
"""

import sys


def count(n):
    for i in range(n):
        yield i * 2


def two_phase(a, b):
    yield a
    yield b
    yield a + b


def with_default(n, step=3):
    for i in range(n):
        yield i * step


def echo(start):
    got = start
    while True:
        got = yield got
        if got is None:
            return "done"


def guarded(n):
    try:
        for i in range(n):
            yield i
    finally:
        print("guarded finally")


def noisy(tag):
    print("noisy body starts", tag)
    yield tag
    print("noisy body ends", tag)


class Tracked:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        print("del", self.name)


def holds(obj):
    yield obj.name
    yield obj.name + "!"


def raises_midway(n):
    for i in range(n):
        if i == 3:
            raise ValueError("at three")
        yield i


# Fused reductions: the loop is the generator's only consumer.
def sum_loop(n):
    total = 0
    for v in count(n):
        total += v
    return total


def prod_loop(n):
    total = 1
    for v in count(n):
        total *= v + 1
    return total


def min_loop(n):
    best = 10**9
    for v in count(n):
        if v < best:
            best = v
    return best


def max_loop(n):
    best = -1
    for v in count(n):
        if v > best:
            best = v
    return best


def float_sum(n):
    total = 0.5
    for v in count(n):
        total += v
    return total


def break_loop(n):
    seen = []
    for v in count(n):
        if v > 6:
            break
        seen.append(v)
    return seen


def return_in_loop(n):
    for v in count(n):
        if v == 4:
            return "found"
    return "missing"


def else_loop(n):
    for v in count(n):
        pass
    else:
        return "exhausted"
    return "broken"


def nested(n):
    pairs = []
    for a in count(n):
        for b in two_phase(a, 1):
            pairs.append((a, b))
    return pairs


def multi_yield():
    return [v for v in two_phase(3, 4)]


def loop_target_survives(n):
    v = "unset"
    for v in count(n):
        pass
    return v


def empty_loop():
    v = "unset"
    total = 7
    for v in count(0):
        total += v
    return v, total


def raise_in_loop():
    seen = []
    try:
        for v in raises_midway(10):
            seen.append(v)
    except ValueError as exc:
        return seen, str(exc)
    return seen, None


def finally_on_break():
    for v in guarded(5):
        if v == 1:
            break
    print("after break loop")


def finally_on_exhaustion():
    for v in guarded(2):
        pass
    print("after full loop")


# Escapes: the generator object is observable.
def escapes():
    g = count(3)
    print(type(g).__name__, g.__name__, g.__qualname__)
    print(next(g), next(g), next(g))
    try:
        next(g)
    except StopIteration:
        print("stop")
    print(list(count(4)), tuple(count(2)), sum(count(5)), max(count(5)))
    print(iter(g) is g)


def send_and_close():
    g = echo(1)
    print(next(g), g.send(5), g.send(9))
    try:
        g.send(None)
    except StopIteration as stop:
        print("returned", stop.value)
    h = guarded(3)
    print(next(h))
    h.close()
    print("closed")
    k = echo(0)
    next(k)
    try:
        k.throw(KeyError("thrown"))
    except KeyError as exc:
        print("threw", exc)


def lazy_start():
    g = noisy("a")
    print("created")
    for v in g:
        print("got", v)
    for v in noisy("b"):
        print("got", v)


def argument_errors():
    try:
        count()
    except TypeError as exc:
        print("TypeError", "missing" in str(exc))
    try:
        count(1, 2)
    except TypeError as exc:
        print("TypeError", "positional" in str(exc))
    try:
        for v in count("x"):
            print(v)
    except TypeError as exc:
        print("TypeError in body", type(exc).__name__)


def lifetime():
    for name in holds(Tracked("fused")):
        print(name)
    print("after fused")
    g = holds(Tracked("escaped"))
    print(next(g))
    del g
    print("after escaped")


def defaults_and_keywords():
    print(list(with_default(3)), list(with_default(3, 2)), list(with_default(n=2, step=5)))
    total = 0
    for v in with_default(4):
        total += v
    print(total)
    args = (3,)
    print(sum(count(*args)))


def swapped(n):
    for i in range(n):
        yield i + 100


def swapped_loop(n):
    out = []
    for v in swapped(n):
        out.append(v)
    return out


def rebound():
    # A writer of `swapped` exists, so no call of it is proven; `count` has
    # none and stays proven.
    global swapped
    original = swapped

    def replacement(n):
        yield "rebound"
        yield n

    print(swapped_loop(2))
    swapped = replacement
    try:
        print(swapped_loop(2))
    finally:
        swapped = original
    print(swapped_loop(1), sum_loop(4))


def renamed():
    yield 1


renamed.__name__ = "new_name"
renamed.__qualname__ = "Outer.new_name"


def names():
    # A generator takes its names from its function when it is created; a
    # proven call and a call through the function object agree.
    def inner():
        yield 0

    for g in (count(1), with_default(1, step=2), inner(), renamed(), swapped(1)):
        print(g.__name__, g.__qualname__)
    g = count(2)
    g.__name__ = "assigned"
    g.__qualname__ = "Assigned.qualname"
    print(g.__name__, g.__qualname__, list(g), g.__name__)
    print(count(1).__name__, count.__name__)
    for attribute in ("__name__", "__qualname__"):
        for value in (3, None, b"bytes"):
            try:
                setattr(g, attribute, value)
            except TypeError as exc:
                print("TypeError", exc)
        try:
            delattr(g, attribute)
        except TypeError as exc:
            print("TypeError", exc)
    print(g.__name__, g.__qualname__)


def main():
    print(sum_loop(10), sum_loop(0), sum_loop(200000))
    print(prod_loop(6), min_loop(5), max_loop(5), float_sum(4))
    print(sum_loop(3) + 2**70)
    print(break_loop(10), return_in_loop(10), return_in_loop(2), else_loop(3))
    print(nested(3))
    print(multi_yield(), loop_target_survives(4), empty_loop())
    print(raise_in_loop())
    finally_on_break()
    finally_on_exhaustion()
    escapes()
    send_and_close()
    lazy_start()
    argument_errors()
    lifetime()
    defaults_and_keywords()
    rebound()
    names()
    sys.stdout.flush()


main()
