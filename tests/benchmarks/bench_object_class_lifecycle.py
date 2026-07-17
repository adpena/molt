"""Bare-object and user-class allocation/refcount lifecycle benchmark."""

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


def allocate_bare_objects(iterations: int) -> int:
    hits = 0
    for _ in range(iterations):
        value = object()
        hits += type(value) is object
    # Make the final iteration's lifetime explicit before the epoch endpoint.
    del value
    return hits


def allocate_plain_instances(iterations: int) -> int:
    hits = 0
    for _ in range(iterations):
        value = Plain()
        hits += type(value) is Plain
    del value
    return hits


def main() -> None:
    # Warm constructor dispatch and allocation classes outside measured epochs.
    hits = allocate_bare_objects(10_000)
    hits += allocate_plain_instances(10_000)

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("bare_object_lifecycle")
    hits += allocate_bare_objects(1_000_000)
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    if _profile_epoch_reset is not None:
        _profile_epoch_reset("user_class_lifecycle")
    hits += allocate_plain_instances(1_000_000)
    if _profile_epoch_dump is not None:
        _profile_epoch_dump()

    print(hits - 20_000)


if __name__ == "__main__":
    main()
