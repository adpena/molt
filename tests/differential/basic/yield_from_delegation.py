"""Purpose: differential coverage for yield-from delegation (throw/close)."""


events = []


def subgen():
    try:
        yield "sub"
    except ValueError:
        events.append("sub_throw")
        raise
    finally:
        events.append("sub_final")


def outer():
    try:
        yield from subgen()
    finally:
        events.append("outer_final")


g = outer()
print("first", next(g))
try:
    g.throw(ValueError("boom"))
except ValueError:
    print("thrown")
print("events1", events)


events = []


class SubIter:
    def __init__(self):
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.i == 0:
            self.i += 1
            return "iter"
        raise StopIteration("done")

    def close(self):
        events.append("sub_close")


def outer2():
    try:
        yield from SubIter()
    finally:
        events.append("outer_final")


g2 = outer2()
print("first2", next(g2))
g2.close()
print("events2", events)


# An exception that leaves `yield from` ends the delegation: gi_yieldfrom is
# None afterwards, and a later throw or close reaches the delegating frame.
class Failing:
    def __iter__(self):
        return self

    def __next__(self):
        raise KeyError("sub")

    def close(self):
        print("stale delegate closed")


def recovers(sub):
    try:
        yield from sub
    except KeyError:
        yield "recovered"
    yield "after"


def first_then_fail():
    yield "inner"
    raise LookupError("inner failed")


def recovers_from_generator():
    try:
        yield from first_then_fail()
    except LookupError:
        yield "recovered"


stale = recovers(Failing())
print("stale-first", next(stale), stale.gi_yieldfrom)
stale.close()
print("stale-closed", stale.gi_frame is None)
resumed = recovers_from_generator()
print("resumed-first", next(resumed), type(resumed.gi_yieldfrom).__name__)
print("resumed-second", next(resumed), resumed.gi_yieldfrom)
try:
    resumed.throw(ValueError("outer"))
except ValueError as error:
    print("resumed-throw", error)
