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
