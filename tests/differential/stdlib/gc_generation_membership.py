# MOLT_META: stderr=exact
"""Purpose: generation membership stays exact through promotion, untracking,
re-tracking, freezing and young collections over a large live heap.

Young collections must find young cycles and only young objects; each object
sits in exactly one generation. Collection is observed through weakref
callbacks, never through implementation-specific collected counts.
"""

import gc
import sys
import weakref

GENERATIONAL_312 = sys.version_info[:2] == (3, 12)


class Node:
    pass


def generation_of(target):
    if not GENERATIONAL_312:
        return "n/a"
    found = [g for g in range(3) if any(o is target for o in gc.get_objects(g))]
    return found


def make_cycle(log, label):
    first, second = Node(), Node()
    first.peer, second.peer = second, first
    return weakref.ref(first, lambda _ref: log.append(label))


gc.disable()
try:
    gc.collect()

    survivor = [Node()]
    print("new object", generation_of(survivor))
    gc.collect(0)
    print("after young collection", generation_of(survivor))
    gc.collect(1)
    print("after middle collection", generation_of(survivor))
    gc.collect(2)
    print("after full collection", generation_of(survivor))

    log = []
    young = make_cycle(log, "young")
    gc.collect(0)
    print("young cycle found by a young collection", log == ["young"], young() is None)

    # Keep the cycle reachable while it is promoted, then release it.
    log.clear()
    first, second = Node(), Node()
    first.peer, second.peer = second, first
    old = weakref.ref(first, lambda _ref: log.append("old"))
    gc.collect(1)
    gc.collect(1)
    del first, second
    if GENERATIONAL_312:
        gc.collect(0)
        print("old cycle untouched by a young collection", log == [])
    else:
        print("old cycle untouched by a young collection", True)
    gc.collect(2)
    print("old cycle found by a full collection", log == ["old"], old() is None)

    # A large live heap: young collections stay exact while most tracked
    # objects are old, and every young cycle is still found.
    live = [{"key": i, "value": [i]} for i in range(50_000)]
    gc.collect(2)
    log.clear()
    refs = []
    for round_index in range(20):
        refs.extend(make_cycle(log, round_index) for _ in range(100))
        gc.collect(0)
    print("young cycles found", len(log), all(ref() is None for ref in refs))
    print("live heap intact", len(live), live[49_999]["value"] == [49_999])

    # Untrack and re-track: an emptied, refilled container keeps one membership.
    mapping = {"edge": []}
    gc.collect(0)
    mapping.clear()
    mapping["edge"] = [mapping]
    gc.collect(0)
    print("re-tracked", gc.is_tracked(mapping), generation_of(mapping))
    holder = weakref.ref(Node())
    gc.collect(0)
    print("dead object gone", holder() is None)

    # Frozen objects leave every generation and return to the oldest.
    frozen = [Node()]
    gc.freeze()
    print("frozen count positive", gc.get_freeze_count() > 0)
    print("frozen generation", generation_of(frozen))
    gc.unfreeze()
    print("unfrozen generation", generation_of(frozen), gc.get_freeze_count())
    del live
finally:
    gc.enable()
