"""Purpose: differential coverage for GC cycle collection."""

try:
    import gc
    import weakref
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    class Node:
        def __init__(self) -> None:
            self.next = None

    def make_cycle() -> weakref.ref:
        first = Node()
        second = Node()
        first.next = second
        second.next = first
        return weakref.ref(first)

    ref = make_cycle()
    collected = gc.collect()
    print(isinstance(collected, int))
    print(ref() is None)
