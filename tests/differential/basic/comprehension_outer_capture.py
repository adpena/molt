"""Purpose: differential coverage for nested comprehension capture of outer vars."""

import sys

vals = [[i + j for j in range(2)] for i in range(2)]
print("vals", vals)

funcs = [(lambda: i) for i in range(3)]
print("late", [f() for f in funcs])

# Outer acquisition is eager, whereas nested iterables and captured bindings
# are read when iteration reaches them. Namespace callbacks cannot redirect the
# hidden frame parameter, even when user names resemble compiler temporaries.
events = []


class Values:
    def __iter__(self):
        events.append("iter")
        return iter((2, 3))


original_source = Values()
globals()["source"] = original_source
__molt_genexpr_outer_iter_1 = "user binding"
gen = (x for x in source)
outer_parameter = gen.gi_frame.f_locals[".0"]
print(
    "outer-parameter",
    outer_parameter is original_source,
    type(outer_parameter).__name__,
    gen.gi_code.co_argcount,
    gen.gi_code.co_posonlyargcount,
)
if sys.version_info >= (3, 15):
    gen.gi_frame.f_locals[".0"] = ()
source = ()
print("acquired", events)
print("consumed", list(gen), events, __molt_genexpr_outer_iter_1)


def nested():
    first = Values()
    offset = 10
    inner = [1]
    pending = (x + y + offset for x in first for y in inner)
    offset = 20
    inner = [4, 5]
    return pending


print("nested", list(nested()))


class Owner:
    source = (4, 5)
    pending = (value for value in source)


print("class", list(Owner.pending))


if sys.version_info >= (3, 15):
    retired = []

    class TrackedIterator:
        def __init__(self, remaining):
            self.remaining = remaining

        def __iter__(self):
            return self

        def __next__(self):
            if not self.remaining:
                raise StopIteration
            self.remaining -= 1
            return self.remaining

        def __del__(self):
            retired.append("iterator")

    class TrackedIterable:
        def __init__(self, count):
            self.count = count

        def __iter__(self):
            return TrackedIterator(self.count)

    closed = (item for item in TrackedIterable(0))
    closed.close()
    print("closed-iterator", retired, closed.gi_frame is None)
    retired.clear()

    exhausted = (item for item in TrackedIterable(1))
    print("exhausted-iterator", list(exhausted), retired, exhausted.gi_frame is None)
    retired.clear()

    suspended = (item for item in TrackedIterable(1))
    print("suspended-iterator", next(suspended), retired)
    suspended.close()
    print("closed-suspended", retired, suspended.gi_frame is None)
