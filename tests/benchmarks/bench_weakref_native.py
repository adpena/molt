"""Runtime-native weakref construction/cache/call/hash microbenchmark.

Run through the standard benchmark memory guard.  After the first exact
callback-free ``ref`` construction, the construction loop must hit the same
runtime object: no class build, Python metaclass dispatch, or weakref allocation.
The call and sticky-hash loops exercise the native protocol and should allocate
nothing after warmup.  Peak/live allocation counters from the guarded harness
are the allocation authority; elapsed time alone is insufficient.
"""

import sys
import weakref

if sys.implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _profile_epoch_reset = load_intrinsic("molt_profile_epoch_reset")
    _profile_epoch_dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _profile_epoch_reset = None
    _profile_epoch_dump = None


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

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("weakref_constructor_cache_hits")
    for _ in range(1_000_000):
        identity_hits += weakref.ref(target) is reference
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("weakref_calls")
    for _ in range(1_000_000):
        call_hits += reference() is target
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("weakref_sticky_hash_hits")
    for _ in range(1_000_000):
        hash_total += hash(reference)
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    print(identity_hits, call_hits, hash_total, expected_hash)


if __name__ == "__main__":
    main()
