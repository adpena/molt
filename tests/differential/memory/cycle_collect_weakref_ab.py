# MOLT_ENV: MOLT_ASSERT_NO_LEAK=1 MOLT_LEAK_TOLERANCE=8
"""Weakref oracle for explicit cyclic collection."""

import gc
import weakref


class Node:
    def __init__(self) -> None:
        self.peer = None


def build_unreachable_cycle():
    a = Node()
    b = Node()
    a.peer = b
    b.peer = a
    return weakref.ref(a), weakref.ref(b)


wa, wb = build_unreachable_cycle()
collected = gc.collect()
print("collected_is_int:", isinstance(collected, int))
print("wa() is None:", wa() is None)
print("wb() is None:", wb() is None)
print("cycle reclaimed:", wa() is None and wb() is None)
if wa() is not None or wb() is not None:
    raise AssertionError("cyclic weakref referents survived gc.collect()")
