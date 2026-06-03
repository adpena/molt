"""Purpose: an iterator that raises mid-iteration must propagate (not hang) when
consumed by an eager container builder *inside a try block*.

Regression for a frontend bug: `_emit_iter_next_checked` skipped the
pending-exception check whenever a try block was active, so the hand-rolled
list/tuple/set/frozenset/dict (and comprehension) consumption loops — which only
test the done-flag — spun forever appending garbage when ITER_NEXT raised
mid-iteration (e.g. zip(strict=True) mismatch, or a user __next__ that raises).
The fix routes the pending exception to the active try handler, exactly like the
sibling `_emit_iter_new` helper. All exception messages here are version-stable
across CPython 3.12/3.13/3.14.
"""


def show(label, fn):
    try:
        r = fn()
        print(label, "OK", repr(r))
    except Exception as e:
        print(label, type(e).__name__, str(e))


class Boom:
    """Yields n descending values, then raises (never StopIteration)."""

    def __init__(self, n, exc=ValueError, msg="boom"):
        self.n = n
        self.exc = exc
        self.msg = msg

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise self.exc(self.msg)
        self.n -= 1
        return self.n


class BoomPairs:
    """Yields n (k, v) pairs, then raises (for dict() / dict-comp)."""

    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise ValueError("boom-pairs")
        self.n -= 1
        return (self.n, self.n * 10)


# Eager builders over a user iterator that raises mid-iteration, inside try.
show("list", lambda: list(Boom(2)))
show("tuple", lambda: tuple(Boom(2)))
show("set", lambda: set(Boom(2)))
show("frozenset", lambda: frozenset(Boom(2)))
show("dict", lambda: dict(BoomPairs(2)))
show("sorted", lambda: sorted(Boom(2)))

# Comprehensions over the same raising iterator, inside try.
show("listcomp", lambda: [x for x in Boom(2)])
show("setcomp", lambda: {x for x in Boom(2)})
show("dictcomp", lambda: {k: v for k, v in BoomPairs(2)})

# A different exception type still propagates with the right class + message.
show("list_typeerr", lambda: list(Boom(1, TypeError, "nope")))

# zip(strict=True) raises a *runtime-internal* ValueError mid-iteration.
show("zip_list", lambda: list(zip([1, 2], [1], strict=True)))
show("zip_set", lambda: set(zip([1, 2], [1], strict=True)))


# Function-exit propagation: the builder is NOT in a try at its own frame, so
# the exception must exit make_list() and reach show()'s try one frame up.
def make_list():
    return list(Boom(2))


show("via_func", make_list)


# Nested try: the exception must route to the *innermost* active handler.
def nested():
    try:
        try:
            return list(Boom(2))
        except ValueError as e:
            return ("inner", str(e))
    except ValueError as e:
        return ("outer", str(e))


show("nested", nested)

print("DONE")
