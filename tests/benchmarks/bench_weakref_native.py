"""Runtime-native weakref construction/cache/call/hash microbenchmark.

Run through the standard benchmark memory guard.  After the first exact
callback-free ``ref`` construction, the construction loop must hit the same
runtime object: no class build, Python metaclass dispatch, or weakref allocation.
The call and sticky-hash loops exercise the native protocol and should allocate
nothing after warmup.  Peak/live allocation counters from the guarded harness
are the allocation authority; elapsed time alone is insufficient.
"""

import weakref


class Target:
    __slots__ = ("value", "__weakref__")

    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return self.value


def main() -> None:
    target = Target(41)
    reference = weakref.ref(target)
    expected_hash = hash(reference)
    identity_hits = 0
    call_hits = 0
    hash_total = 0

    for _ in range(1_000_000):
        identity_hits += weakref.ref(target) is reference
    for _ in range(1_000_000):
        call_hits += reference() is target
    for _ in range(1_000_000):
        hash_total += hash(reference)

    print(identity_hits, call_hits, hash_total, expected_hash)


if __name__ == "__main__":
    main()
