"""Bare-object and user-class allocation/refcount lifecycle benchmark.

The two warmed epochs exercise the shared class-edge constructor authority.
Every iteration allocates, validates the runtime class, and drops the only
owning instance reference. Runtime epoch counters distinguish balanced
allocation/deallocation from a fast leak; the guarded runner records wall time
and process-tree peak RSS.
"""

import sys

if sys.implementation.name == "molt":
    from _intrinsics import load_intrinsic

    _profile_epoch_reset = load_intrinsic("molt_profile_epoch_reset")
    _profile_epoch_dump = load_intrinsic("molt_profile_epoch_dump")
else:
    _profile_epoch_reset = None
    _profile_epoch_dump = None


class Plain:
    __slots__ = ()


def main() -> None:
    hits = 0

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("bare_object_lifecycle")
    for _ in range(1_000_000):
        value = object()
        hits += type(value) is object
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("user_class_lifecycle")
    for _ in range(1_000_000):
        value = Plain()
        hits += type(value) is Plain
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    print(hits)


if __name__ == "__main__":
    main()
