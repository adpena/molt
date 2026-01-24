"""Purpose: differential coverage for generator introspection attributes."""

import inspect


def locals_view(gen):
    local_map = inspect.getgeneratorlocals(gen)
    return (sorted(local_map.keys()), local_map.get("x"))


def sub():
    yield "sub-1"
    yield "sub-2"


def outer():
    x = 1
    yield x
    yield from sub()
    return "done"


g = outer()
print("code", g.gi_code.co_name)
print("frame0_none", g.gi_frame is None)
print("frame0_code", g.gi_frame.f_code.co_name)
print("running0", g.gi_running)
print("yieldfrom0_none", g.gi_yieldfrom is None)
print("locals0", locals_view(g))
print("next1", next(g))
print("frame1_none", g.gi_frame is None)
print("frame1_code", g.gi_frame.f_code.co_name)
print("running1", g.gi_running)
print("yieldfrom1_none", g.gi_yieldfrom is None)
print("locals1", locals_view(g))
print("next2", next(g))
print(
    "yieldfrom2_type",
    type(g.gi_yieldfrom).__name__ if g.gi_yieldfrom is not None else None,
)
print("running2", g.gi_running)
print("locals2", locals_view(g))
print("next3", next(g))
print("yieldfrom3_none", g.gi_yieldfrom is None)
try:
    next(g)
except StopIteration as exc:
    print("stop", exc.value)
print("frame_end", g.gi_frame is None)
print("yieldfrom_end", g.gi_yieldfrom is None)
print("locals_end", locals_view(g))
