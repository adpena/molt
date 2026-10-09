"""Purpose: differential coverage for freeing an exception a dead generator caught."""

import gc
import weakref


class Boom(Exception):
    pass


refs = []


def gen():
    try:
        raise Boom("boom")
    except Boom as exc:
        refs.append(weakref.ref(exc))
        yield "step"


g = gen()
print(next(g))
g = None
gc.collect()
print("cleared", refs[0]() is None)
